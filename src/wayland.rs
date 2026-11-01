//! Native Wayland frontend: a wlr-layer-shell overlay surface rendered with egui + wgpu.
//!
//! The launcher is a transparent overlay covering the output, with the actual window drawn
//! centered and shifted upwards. This gives exact positioning (which xdg-toplevels can't do),
//! exclusive keyboard focus and click-outside-to-close.

use std::os::unix::net::UnixListener;
use std::ptr::NonNull;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle,
    WaylandWindowHandle,
};
use smithay_client_toolkit::{
    activation::{ActivationHandler, ActivationState, RequestData},
    compositor::{CompositorHandler, CompositorState},
    delegate_activation, delegate_compositor, delegate_keyboard, delegate_layer, delegate_output,
    delegate_pointer, delegate_registry, delegate_seat, delegate_shm,
    output::{OutputHandler, OutputState},
    reexports::calloop::{
        self, EventLoop, LoopHandle, RegistrationToken,
        channel::{Channel, Event as ChannelEvent, Sender},
        generic::Generic,
        timer::{TimeoutAction, Timer},
    },
    reexports::calloop_wayland_source::WaylandSource,
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers},
        pointer::{CursorIcon, PointerEvent, PointerEventKind, PointerHandler, ThemeSpec, ThemedPointer},
    },
    shell::{
        WaylandSurface,
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
    },
    shm::{Shm, ShmHandler, raw::RawPool},
};
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle,
    globals::{GlobalList, registry_queue_init},
    protocol::{wl_buffer, wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
};
use wayland_protocols_plasma::output_order::v1::client::kde_output_order_v1::{self, KdeOutputOrderV1};
use wayland_protocols::wp::fractional_scale::v1::client::{
    wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
    wp_fractional_scale_v1::{self, WpFractionalScaleV1},
};
use wayland_protocols::wp::viewporter::client::{wp_viewport::WpViewport, wp_viewporter::WpViewporter};

use crate::apps::{self, App};
use crate::config::{APP_ID, Monitor};
use crate::icons::LoadedIcon;
use crate::ipc::{self, Command};
use crate::ui::{Launcher, NavKey, UiEvent};

/// Messages from background threads into the event loop.
pub enum Msg {
    Command(Command, Option<String>),
    Icon(LoadedIcon),
    /// Result of an app scan, tagged with the scan's sequence number.
    Apps(u64, Vec<App>),
    /// Something shown in the window changed in a background thread.
    Redraw,
    /// A desktop entry was added, changed or removed.
    AppsChanged,
}

/// Wrapper so wgpu can open the EGL/Vulkan display for our Wayland connection.
#[derive(Debug)]
struct WaylandDisplay(NonNull<std::ffi::c_void>);
// SAFETY: the pointer is the libwayland display which outlives the wgpu instance and is
// only used by wgpu to create surfaces.
unsafe impl Send for WaylandDisplay {}
unsafe impl Sync for WaylandDisplay {}

impl HasDisplayHandle for WaylandDisplay {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        let raw = RawDisplayHandle::Wayland(WaylandDisplayHandle::new(self.0));
        Ok(unsafe { DisplayHandle::borrow_raw(raw) })
    }
}

struct Gpu {
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: egui_wgpu::Renderer,
    format: wgpu::TextureFormat,
}

struct Window {
    layer: LayerSurface,
    surface: Option<wgpu::Surface<'static>>,
    fractional: Option<WpFractionalScaleV1>,
    viewport: Option<WpViewport>,
    /// Logical size.
    width: u32,
    height: u32,
    scale: f64,
    /// Integer scale used when fractional scaling is unavailable.
    buffer_scale: i32,
    configured: bool,
    needs_reconfigure: bool,
    frame_pending: bool,
}

impl Window {
    fn physical_size(&self) -> (u32, u32) {
        (
            ((self.width as f64) * self.scale).round().max(1.0) as u32,
            ((self.height as f64) * self.scale).round().max(1.0) as u32,
        )
    }
}

/// Invisible full-screen surfaces on every output, used to find out which output the
/// pointer is on (Wayland clients can't query the global pointer position).
struct Probe {
    surfaces: Vec<ProbeSurface>,
    _pool: RawPool,
    timer: Option<RegistrationToken>,
}

struct ProbeSurface {
    layer: LayerSurface,
    output: wl_output::WlOutput,
    viewport: WpViewport,
    buffer: wl_buffer::WlBuffer,
}

/// How long to wait for the pointer to enter a probe surface.
const PROBE_TIMEOUT: Duration = Duration::from_millis(150);

struct PendingLaunch {
    spec: apps::LaunchSpec,
    started: Instant,
}

pub struct State {
    conn: Connection,
    qh: QueueHandle<State>,
    loop_handle: LoopHandle<'static, State>,
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    compositor: CompositorState,
    layer_shell: LayerShell,
    shm: Shm,
    activation: Option<ActivationState>,
    fractional_manager: Option<WpFractionalScaleManagerV1>,
    viewporter: Option<WpViewporter>,

    seat: Option<wl_seat::WlSeat>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    pointer: Option<ThemedPointer>,
    modifiers: Modifiers,
    last_serial: u32,
    keyboard_focus: bool,

    instance: wgpu::Instance,
    gpu: Option<Gpu>,
    window: Option<Window>,
    probe: Option<Probe>,
    /// KDE's output order (first = primary), from kde_output_order_v1.
    _output_order_global: Option<KdeOutputOrderV1>,
    output_order: Vec<String>,
    output_order_pending: Vec<String>,

    egui_ctx: egui::Context,
    egui_events: Vec<egui::Event>,
    start_time: Instant,
    pointer_pos: Option<egui::Pos2>,
    redraw: bool,
    repaint_timer: Option<RegistrationToken>,
    clipboard: smithay_clipboard::Clipboard,

    launcher: Launcher,
    msg_tx: Sender<Msg>,
    hotkey: crate::hotkey::Hotkey,
    scan_seq: u64,
    applied_scan: u64,
    pending_launch: Option<PendingLaunch>,
    /// Debounces toggle commands that arrive twice (e.g. portal + compositor binding).
    last_toggle: Option<Instant>,
    exit: bool,
}

pub fn run(listener: UnixListener, show_on_start: bool) -> anyhow::Result<()> {
    let conn = Connection::connect_to_env().context("cannot connect to the Wayland compositor")?;
    let (globals, event_queue) = registry_queue_init::<State>(&conn)?;
    let qh = event_queue.handle();

    let mut event_loop: EventLoop<'static, State> = EventLoop::try_new()?;
    let loop_handle = event_loop.handle();
    WaylandSource::new(conn.clone(), event_queue)
        .insert(loop_handle.clone())
        .map_err(|e| anyhow::anyhow!("cannot insert wayland source: {e}"))?;

    let compositor = CompositorState::bind(&globals, &qh).context("wl_compositor not available")?;
    let layer_shell = LayerShell::bind(&globals, &qh).context(
        "the compositor does not support wlr-layer-shell (supported: KDE, Sway, Hyprland, niri, COSMIC, …)",
    )?;
    let shm = Shm::bind(&globals, &qh).context("wl_shm not available")?;
    let activation = ActivationState::bind(&globals, &qh).ok();
    let fractional_manager = bind_optional::<WpFractionalScaleManagerV1>(&globals, &qh);
    let viewporter = bind_optional::<WpViewporter>(&globals, &qh);
    let output_order_global = bind_optional::<KdeOutputOrderV1>(&globals, &qh);

    let display_ptr = NonNull::new(conn.backend().display_ptr() as *mut std::ffi::c_void)
        .context("null wayland display")?;
    let mut desc = wgpu::InstanceDescriptor::new_with_display_handle(Box::new(WaylandDisplay(display_ptr)));
    desc.backends = wgpu::Backends::VULKAN | wgpu::Backends::GL;
    let instance = wgpu::Instance::new(desc);

    let clipboard = unsafe { smithay_clipboard::Clipboard::new(conn.backend().display_ptr() as *mut _) };

    // Background threads talk to the loop through this channel.
    let (msg_tx, msg_rx): (Sender<Msg>, Channel<Msg>) = calloop::channel::channel();
    loop_handle
        .insert_source(msg_rx, |event, _, state: &mut State| {
            if let ChannelEvent::Msg(msg) = event {
                state.handle_msg(msg);
            }
        })
        .map_err(|e| anyhow::anyhow!("cannot insert channel: {e}"))?;

    // Control socket
    listener.set_nonblocking(true)?;
    let tx = msg_tx.clone();
    loop_handle
        .insert_source(
            Generic::new(listener, calloop::Interest::READ, calloop::Mode::Level),
            move |_, listener, _state: &mut State| {
                while let Ok((stream, _)) = listener.accept() {
                    if let Some((cmd, query)) = ipc::read_command(stream) {
                        let _ = tx.send(Msg::Command(cmd, query));
                    }
                }
                Ok(calloop::PostAction::Continue)
            },
        )
        .map_err(|e| anyhow::anyhow!("cannot insert socket: {e}"))?;

    let icon_tx = msg_tx.clone();
    let mut launcher = Launcher::new(move |icon| {
        let _ = icon_tx.send(Msg::Icon(icon));
    });

    let (toggle_tx, status_tx) = (msg_tx.clone(), msg_tx.clone());
    let hotkey = crate::hotkey::spawn(
        launcher.config.hotkey.clone(),
        launcher.config.hotkey_enabled,
        move || {
            let _ = toggle_tx.send(Msg::Command(Command::Toggle, None));
        },
        move || {
            let _ = status_tx.send(Msg::Redraw);
        },
    );
    launcher.set_hotkey_status(hotkey.status.clone());

    let egui_ctx = egui::Context::default();
    let mut state = State {
        registry_state: RegistryState::new(&globals),
        seat_state: SeatState::new(&globals, &qh),
        output_state: OutputState::new(&globals, &qh),
        conn,
        qh,
        loop_handle,
        compositor,
        layer_shell,
        shm,
        activation,
        fractional_manager,
        viewporter,
        seat: None,
        keyboard: None,
        pointer: None,
        modifiers: Modifiers::default(),
        last_serial: 0,
        keyboard_focus: false,
        instance,
        gpu: None,
        window: None,
        probe: None,
        _output_order_global: output_order_global,
        output_order: Vec::new(),
        output_order_pending: Vec::new(),
        egui_ctx,
        egui_events: Vec::new(),
        start_time: Instant::now(),
        pointer_pos: None,
        redraw: false,
        repaint_timer: None,
        clipboard,
        launcher,
        msg_tx,
        hotkey,
        scan_seq: 0,
        applied_scan: 0,
        pending_launch: None,
        last_toggle: None,
        exit: false,
    };
    state.launcher.setup(&state.egui_ctx);
    state.rescan_apps();
    // Live updates of the app list while running (and while the launcher is open).
    let tx = state.msg_tx.clone();
    apps::watch(move || {
        let _ = tx.send(Msg::AppsChanged);
    });

    if show_on_start {
        state.show();
    }
    log::info!("rlauncher running (socket {})", ipc::socket_path().display());

    while !state.exit {
        event_loop.dispatch(None, &mut state)?;
        if state.redraw {
            state.redraw = false;
            state.draw();
        }
    }
    Ok(())
}

fn bind_optional<I>(globals: &GlobalList, qh: &QueueHandle<State>) -> Option<I>
where
    I: Proxy + 'static,
    State: Dispatch<I, ()>,
{
    globals.bind::<I, State, ()>(qh, 1..=1, ()).ok()
}

impl State {
    fn handle_msg(&mut self, msg: Msg) {
        match msg {
            Msg::Command(cmd, query) => self.handle_command(cmd, query),
            Msg::Icon(icon) => {
                self.launcher.icon_loaded(icon);
                self.redraw = self.window.is_some();
            }
            Msg::Apps(seq, apps) => {
                // scans run in parallel threads: never let an older one win
                if seq > self.applied_scan {
                    self.applied_scan = seq;
                    self.launcher.set_apps(apps);
                    self.redraw = self.window.is_some();
                }
            }
            Msg::Redraw => self.redraw = self.window.is_some(),
            Msg::AppsChanged => {
                log::debug!("application folders changed, rescanning");
                self.rescan_apps();
            }
        }
    }

    fn handle_command(&mut self, cmd: Command, query: Option<String>) {
        log::debug!("command: {cmd:?}");
        match cmd {
            Command::Toggle => {
                // Ignore a second toggle within a few ms (two bindings for the same key).
                if self.last_toggle.is_some_and(|t| t.elapsed() < Duration::from_millis(150)) {
                    return;
                }
                self.last_toggle = Some(Instant::now());
                if self.window.is_some() || self.probe.is_some() {
                    self.hide();
                } else {
                    self.show();
                }
            }
            Command::Show => {
                self.show();
                if let Some(q) = query {
                    self.launcher.set_query(q);
                    self.redraw = true;
                }
            }
            Command::Hide => self.hide(),
            Command::Settings => {
                self.show();
                self.launcher.open_settings(query.as_deref());
                self.redraw = true;
            }
            Command::Reload => {
                self.launcher.reload(&self.egui_ctx);
                self.rescan_apps();
            }
            Command::Quit => {
                self.hide();
                self.exit = true;
            }
        }
    }

    fn rescan_apps(&mut self) {
        self.scan_seq += 1;
        let seq = self.scan_seq;
        let tx = self.msg_tx.clone();
        let blacklist = self.launcher.config.blacklisted_dirs.clone();
        std::thread::spawn(move || {
            let apps = apps::scan(&blacklist);
            log::debug!("found {} applications", apps.len());
            let _ = tx.send(Msg::Apps(seq, apps));
        });
    }

    fn show(&mut self) {
        if self.window.is_some() || self.probe.is_some() {
            return;
        }
        self.launcher.reload(&self.egui_ctx);
        self.launcher.reset();
        self.rescan_apps();

        match self.launcher.config.monitor {
            Monitor::Primary => {
                let output = self.primary_output();
                self.create_window(output.as_ref());
            }
            Monitor::Mouse => {
                if !self.start_probe() {
                    self.create_window(None);
                }
            }
        }
    }

    /// The primary output: first in KDE's output order, otherwise the top-left one.
    /// `None` when there is only one output (the compositor's choice is then the same).
    fn primary_output(&self) -> Option<wl_output::WlOutput> {
        let outputs: Vec<_> = self.output_state.outputs().collect();
        if outputs.len() <= 1 {
            return None;
        }
        let infos: Vec<_> = outputs.iter().map(|o| self.output_state.info(o)).collect();
        for name in &self.output_order {
            if let Some(i) = infos.iter().position(|i| i.as_ref().and_then(|i| i.name.as_ref()) == Some(name)) {
                return Some(outputs[i].clone());
            }
        }
        outputs
            .iter()
            .zip(&infos)
            .min_by_key(|(_, info)| {
                let (x, y) = info.as_ref().map(|i| i.logical_position.unwrap_or(i.location)).unwrap_or((i32::MAX / 4, 0));
                (y.max(0) + x.max(0), y, x)
            })
            .map(|(o, _)| o.clone())
    }

    /// Maps an invisible surface on every output; the pointer entering one of them tells
    /// where the launcher has to go. Returns false if probing is not possible or needed.
    fn start_probe(&mut self) -> bool {
        let outputs: Vec<_> = self.output_state.outputs().collect();
        let Some(viewporter) = self.viewporter.clone() else { return false };
        if outputs.len() <= 1 || self.pointer.is_none() {
            return false;
        }
        let Ok(mut pool) = RawPool::new(4, &self.shm) else { return false };
        let mut surfaces = Vec::new();
        for output in outputs {
            let surface = self.compositor.create_surface(&self.qh);
            let viewport = viewporter.get_viewport(&surface, &self.qh, ());
            let layer = self.layer_shell.create_layer_surface(
                &self.qh,
                surface,
                Layer::Overlay,
                Some("rlauncher-probe"),
                Some(&output),
            );
            layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
            layer.set_exclusive_zone(-1);
            layer.set_keyboard_interactivity(KeyboardInteractivity::None);
            layer.set_size(0, 0);
            layer.commit();
            // a single fully transparent pixel, stretched over the output by the viewport
            let buffer = pool.create_buffer(0, 1, 1, 4, wl_shm::Format::Argb8888, (), &self.qh);
            surfaces.push(ProbeSurface { layer, output, viewport, buffer });
        }
        let timer = self
            .loop_handle
            .insert_source(Timer::from_duration(PROBE_TIMEOUT), |_, _, state: &mut State| {
                if state.probe.is_some() {
                    log::debug!("pointer not found on any output, letting the compositor choose");
                    state.finish_probe(None);
                }
                TimeoutAction::Drop
            })
            .ok();
        self.probe = Some(Probe { surfaces, _pool: pool, timer });
        let _ = self.conn.flush();
        true
    }

    /// Removes the probe surfaces and opens the launcher on `output`.
    fn finish_probe(&mut self, output: Option<wl_output::WlOutput>) {
        if self.cancel_probe() {
            self.create_window(output.as_ref());
        }
    }

    fn cancel_probe(&mut self) -> bool {
        let Some(probe) = self.probe.take() else { return false };
        if let Some(t) = probe.timer {
            self.loop_handle.remove(t);
        }
        for p in probe.surfaces {
            p.viewport.destroy();
            drop(p.layer);
            p.buffer.destroy();
        }
        let _ = self.conn.flush();
        true
    }

    fn create_window(&mut self, output: Option<&wl_output::WlOutput>) {
        if let Some(o) = output {
            log::debug!("opening on output {:?}", self.output_state.info(o).and_then(|i| i.name));
        }
        let surface = self.compositor.create_surface(&self.qh);
        let fractional = self
            .fractional_manager
            .as_ref()
            .map(|m| m.get_fractional_scale(&surface, &self.qh, surface.clone()));
        let viewport = self.viewporter.as_ref().map(|v| v.get_viewport(&surface, &self.qh, ()));
        let layer =
            self.layer_shell
                .create_layer_surface(&self.qh, surface, Layer::Overlay, Some("rlauncher"), output);
        layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
        layer.set_size(0, 0);
        layer.commit();

        self.window = Some(Window {
            layer,
            surface: None,
            fractional,
            viewport,
            width: 0,
            height: 0,
            scale: 1.0,
            buffer_scale: 1,
            configured: false,
            needs_reconfigure: true,
            frame_pending: false,
        });
        self.egui_events.clear();
        self.keyboard_focus = false;
    }

    fn hide(&mut self) {
        self.cancel_probe();
        if self.window.is_some() {
            for cmd in self.launcher.on_hide(&self.egui_ctx) {
                self.hotkey.send(cmd);
            }
        }
        if let Some(mut win) = self.window.take() {
            // The wgpu surface must go before the wl_surface it renders to.
            drop(win.surface.take());
            if let Some(f) = win.fractional.take() {
                f.destroy();
            }
            if let Some(v) = win.viewport.take() {
                v.destroy();
            }
            drop(win);
            let _ = self.conn.flush();
        }
        if let Some(t) = self.repaint_timer.take() {
            self.loop_handle.remove(t);
        }
        self.keyboard_focus = false;
        self.pointer_pos = None;
    }

    fn ensure_gpu(&mut self, surface: &wgpu::Surface<'static>) -> anyhow::Result<()> {
        if self.gpu.is_some() {
            return Ok(());
        }
        let adapter = pollster::block_on(self.instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: Some(surface),
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .context("no suitable GPU adapter")?;
        log::info!("using GPU adapter: {:?}", adapter.get_info().name);
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("rlauncher"),
            required_limits: wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
            ..Default::default()
        }))
        .context("cannot create GPU device")?;
        let caps = surface.get_capabilities(&adapter);
        let format = egui_wgpu::preferred_framebuffer_format(&caps.formats).unwrap_or(caps.formats[0]);
        let renderer = egui_wgpu::Renderer::new(&device, format, egui_wgpu::RendererOptions::default());
        self.gpu = Some(Gpu { adapter, device, queue, renderer, format });
        Ok(())
    }

    fn configure_surface(&mut self) {
        let Some(win) = self.window.as_mut() else { return };
        let (Some(gpu), Some(surface)) = (self.gpu.as_ref(), win.surface.as_ref()) else { return };
        let (w, h) = win.physical_size();
        let caps = surface.get_capabilities(&gpu.adapter);
        let alpha_mode = [wgpu::CompositeAlphaMode::PreMultiplied, wgpu::CompositeAlphaMode::PostMultiplied]
            .into_iter()
            .find(|m| caps.alpha_modes.contains(m))
            .unwrap_or(caps.alpha_modes[0]);
        let present_mode = if caps.present_modes.contains(&wgpu::PresentMode::Mailbox) {
            wgpu::PresentMode::Mailbox
        } else {
            wgpu::PresentMode::Fifo
        };
        surface.configure(
            &gpu.device,
            &wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format: gpu.format,
                width: w,
                height: h,
                present_mode,
                desired_maximum_frame_latency: 2,
                alpha_mode,
                view_formats: vec![],
                color_space: wgpu::SurfaceColorSpace::Auto,
            },
        );
        if let Some(vp) = &win.viewport {
            vp.set_destination(win.width as i32, win.height as i32);
        } else {
            win.layer.wl_surface().set_buffer_scale(win.buffer_scale);
        }
        win.needs_reconfigure = false;
    }

    fn draw(&mut self) {
        let Some(win) = self.window.as_ref() else { return };
        if !win.configured || win.surface.is_none() {
            return;
        }
        if win.needs_reconfigure {
            self.configure_surface();
        }
        let Some(win) = self.window.as_mut() else { return };
        let scale = win.scale as f32;
        let (pw, ph) = win.physical_size();

        let mut raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(win.width as f32, win.height as f32),
            )),
            time: Some(self.start_time.elapsed().as_secs_f64()),
            events: std::mem::take(&mut self.egui_events),
            focused: true,
            max_texture_side: self.gpu.as_ref().map(|g| g.device.limits().max_texture_dimension_2d as usize),
            ..Default::default()
        };
        if let Some(vp) = raw.viewports.get_mut(&egui::ViewportId::ROOT) {
            vp.native_pixels_per_point = Some(scale);
            vp.focused = Some(true);
        }

        let ctx = self.egui_ctx.clone();
        let launcher = &mut self.launcher;
        let mut output = ctx.run_ui(raw, |ui| launcher.ui(ui.ctx()));

        // Clipboard + cursor
        for cmd in &output.platform_output.commands {
            if let egui::OutputCommand::CopyText(text) = cmd {
                self.clipboard.store(text.clone());
            }
        }
        if let Some(p) = &self.pointer {
            let icon = match output.platform_output.cursor_icon {
                egui::CursorIcon::Text => CursorIcon::Text,
                egui::CursorIcon::PointingHand => CursorIcon::Pointer,
                _ => CursorIcon::Default,
            };
            let _ = p.set_cursor(&self.conn, icon);
        }

        let repaint_delay = output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map(|v| v.repaint_delay)
            .unwrap_or(Duration::MAX);

        // Texture uploads happen even if no frame can be presented, egui expects them applied.
        let mut textures = std::mem::take(&mut output.textures_delta);
        let Some(gpu) = self.gpu.as_mut() else {
            textures.clear();
            return;
        };
        for (id, deltas) in textures.set.drain() {
            for delta in &deltas {
                gpu.renderer.update_texture(&gpu.device, &gpu.queue, id, delta);
            }
        }
        let to_free: Vec<egui::TextureId> = textures.free.drain().collect();

        let frame = self
            .window
            .as_ref()
            .and_then(|w| w.surface.as_ref())
            .map(|surface| surface.get_current_texture());
        let frame = match frame {
            Some(wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t)) => t,
            other => {
                log::debug!("cannot acquire frame: {other:?}");
                for id in &to_free {
                    gpu.renderer.free_texture(id);
                }
                if let Some(win) = self.window.as_mut() {
                    win.needs_reconfigure = true;
                    self.redraw = true;
                }
                return;
            }
        };
        let Some(win) = self.window.as_mut() else { return };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let ppp = output.pixels_per_point;
        let primitives = ctx.tessellate(output.shapes, ppp);
        let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: [pw, ph], pixels_per_point: ppp };

        let mut encoder = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        let extra = gpu.renderer.update_buffers(&gpu.device, &gpu.queue, &mut encoder, &primitives, &screen);
        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("egui"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                })
                .forget_lifetime();
            gpu.renderer.render(&mut pass, &primitives, &screen);
        }
        for id in &to_free {
            gpu.renderer.free_texture(id);
        }

        // Ask for a frame callback before presenting (presenting commits the surface).
        let animating = repaint_delay.is_zero();
        if animating && !win.frame_pending {
            let surf = win.layer.wl_surface();
            surf.frame(&self.qh, surf.clone());
            win.frame_pending = true;
        }
        gpu.queue.submit(extra.into_iter().chain(std::iter::once(encoder.finish())));
        gpu.queue.present(frame);

        if !animating && repaint_delay < Duration::from_secs(10) {
            self.schedule_repaint(repaint_delay);
        }
        self.process_ui_events();
    }

    fn schedule_repaint(&mut self, delay: Duration) {
        if let Some(t) = self.repaint_timer.take() {
            self.loop_handle.remove(t);
        }
        let token = self.loop_handle.insert_source(Timer::from_duration(delay), |_, _, state| {
            state.repaint_timer = None;
            state.redraw = state.window.is_some();
            TimeoutAction::Drop
        });
        self.repaint_timer = token.ok();
    }

    fn process_ui_events(&mut self) {
        for ev in std::mem::take(&mut self.launcher.events) {
            match ev {
                UiEvent::Hide => self.hide(),
                UiEvent::Copy(text) => self.clipboard.store(text),
                UiEvent::Launch(spec) => self.launch(spec),
                UiEvent::Hotkey(cmd) => self.hotkey.send(cmd),
                UiEvent::OpenSettings => {
                    self.launcher.open_settings(None);
                    self.redraw = true;
                }
            }
        }
    }

    /// Launches with an XDG activation token so the new window gets focus.
    fn launch(&mut self, spec: apps::LaunchSpec) {
        if self.pending_launch.is_some() {
            return;
        }
        let (Some(activation), Some(win)) = (self.activation.as_ref(), self.window.as_ref()) else {
            apps::spawn(&spec, None, &self.launcher.config.terminal);
            self.hide();
            return;
        };
        activation.request_token(
            &self.qh,
            RequestData {
                app_id: Some(APP_ID.to_string()),
                seat_and_serial: self.seat.clone().map(|s| (s, self.last_serial)),
                surface: Some(win.layer.wl_surface().clone()),
            },
        );
        self.pending_launch = Some(PendingLaunch { spec, started: Instant::now() });
        // Don't wait forever if the compositor never answers.
        let _ = self.loop_handle.insert_source(Timer::from_duration(Duration::from_millis(400)), |_, _, state| {
            if let Some(p) = state.pending_launch.take() {
                log::debug!("no activation token after {:?}, launching without", p.started.elapsed());
                apps::spawn(&p.spec, None, &state.launcher.config.terminal);
                state.hide();
            }
            TimeoutAction::Drop
        });
    }

    fn on_key(&mut self, event: KeyEvent, pressed: bool, repeat: bool) {
        if self.window.is_none() {
            return;
        }
        let mods = self.modifiers;
        if self.launcher.is_recording() {
            // Recording a new global hotkey: the key press is consumed here.
            if pressed && !repeat {
                if event.keysym == Keysym::Escape {
                    self.launcher.settings_escape(&self.egui_ctx);
                } else if let Some(trigger) = crate::hotkey::trigger_from_keysym(
                    event.keysym.raw(),
                    mods.ctrl,
                    mods.alt,
                    mods.shift,
                    mods.logo,
                ) {
                    self.launcher.recorded(trigger);
                }
                self.process_ui_events();
                self.redraw = true;
            }
            return;
        }
        if self.launcher.in_settings() {
            if pressed && event.keysym == Keysym::Escape {
                self.launcher.settings_escape(&self.egui_ctx);
                self.process_ui_events();
                self.redraw = true;
                return;
            }
        } else if pressed && let Some(nav) = self.nav_key(&event, &mods) {
            self.launcher.nav(nav);
            self.redraw = true;
            self.process_ui_events();
            return;
        }
        let emods = egui_modifiers(&mods);
        if let Some(key) = egui_key(event.keysym) {
            if pressed && mods.ctrl && !mods.alt {
                match key {
                    egui::Key::V => {
                        if let Ok(text) = self.clipboard.load() {
                            // multi-line scripts can be pasted into the query editor
                            let text = if self.launcher.in_settings() { text } else { text.replace('\n', " ") };
                            self.egui_events.push(egui::Event::Paste(text));
                        }
                        self.redraw = true;
                        return;
                    }
                    egui::Key::C => self.egui_events.push(egui::Event::Copy),
                    egui::Key::X => self.egui_events.push(egui::Event::Cut),
                    _ => {}
                }
            }
            self.egui_events.push(egui::Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat,
                modifiers: emods,
            });
        }
        if pressed
            && !mods.ctrl
            && !mods.alt
            && !mods.logo
            && let Some(text) = event.utf8.as_ref()
            && !text.is_empty()
            && !text.chars().any(char::is_control)
        {
            self.egui_events.push(egui::Event::Text(text.clone()));
        }
        self.redraw = true;
    }

    fn nav_key(&self, event: &KeyEvent, mods: &Modifiers) -> Option<NavKey> {
        let sym = event.keysym;
        let nav = match sym {
            Keysym::Escape => NavKey::Escape,
            Keysym::Up | Keysym::KP_Up => NavKey::Up,
            Keysym::Down | Keysym::KP_Down => NavKey::Down,
            Keysym::Page_Up => NavKey::PageUp,
            Keysym::Page_Down => NavKey::PageDown,
            Keysym::ISO_Left_Tab => NavKey::Up,
            Keysym::Tab if mods.shift => NavKey::Up,
            Keysym::Tab => NavKey::Complete,
            Keysym::Return | Keysym::KP_Enter => NavKey::Enter { alt: mods.alt },
            _ if mods.ctrl && !mods.alt => match sym {
                Keysym::n | Keysym::j => NavKey::Down,
                Keysym::p | Keysym::k => NavKey::Up,
                _ => return None,
            },
            _ if mods.alt && !mods.ctrl => {
                let c = event
                    .utf8
                    .as_ref()
                    .and_then(|s| s.chars().next())
                    .or_else(|| sym.key_char())?
                    .to_ascii_lowercase();
                let idx = self.launcher.config.jump_keys.chars().position(|k| k == c)?;
                NavKey::Jump(idx)
            }
            _ => return None,
        };
        Some(nav)
    }

    fn is_our_surface(&self, surface: &wl_surface::WlSurface) -> bool {
        self.window.as_ref().is_some_and(|w| w.layer.wl_surface() == surface)
    }

    fn set_scale(&mut self, scale: f64, buffer_scale: i32) {
        if let Some(win) = self.window.as_mut()
            && ((win.scale - scale).abs() > f64::EPSILON || win.buffer_scale != buffer_scale)
        {
            win.scale = scale;
            win.buffer_scale = buffer_scale;
            win.needs_reconfigure = true;
            self.redraw = true;
        }
    }
}

fn egui_modifiers(m: &Modifiers) -> egui::Modifiers {
    egui::Modifiers { alt: m.alt, ctrl: m.ctrl, shift: m.shift, mac_cmd: false, command: m.ctrl }
}

fn egui_key(sym: Keysym) -> Option<egui::Key> {
    use egui::Key;
    Some(match sym {
        Keysym::BackSpace => Key::Backspace,
        Keysym::Delete | Keysym::KP_Delete => Key::Delete,
        Keysym::Left | Keysym::KP_Left => Key::ArrowLeft,
        Keysym::Right | Keysym::KP_Right => Key::ArrowRight,
        Keysym::Up => Key::ArrowUp,
        Keysym::Down => Key::ArrowDown,
        Keysym::Home | Keysym::KP_Home => Key::Home,
        Keysym::End | Keysym::KP_End => Key::End,
        Keysym::Insert => Key::Insert,
        Keysym::Return | Keysym::KP_Enter => Key::Enter,
        Keysym::Tab => Key::Tab,
        Keysym::Escape => Key::Escape,
        Keysym::space => Key::Space,
        Keysym::a | Keysym::A => Key::A,
        Keysym::c | Keysym::C => Key::C,
        Keysym::v | Keysym::V => Key::V,
        Keysym::x | Keysym::X => Key::X,
        Keysym::z | Keysym::Z => Key::Z,
        Keysym::y | Keysym::Y => Key::Y,
        Keysym::w | Keysym::W => Key::W,
        Keysym::u | Keysym::U => Key::U,
        Keysym::h | Keysym::H => Key::H,
        Keysym::k | Keysym::K => Key::K,
        Keysym::e | Keysym::E => Key::E,
        Keysym::b | Keysym::B => Key::B,
        Keysym::f | Keysym::F => Key::F,
        _ => return None,
    })
}

impl CompositorHandler for State {
    fn scale_factor_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, surface: &wl_surface::WlSurface, factor: i32) {
        // Only used when the compositor lacks fractional scaling.
        if self.is_our_surface(surface) && self.fractional_manager.is_none() {
            self.set_scale(factor as f64, factor);
        }
    }

    fn transform_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: wl_output::Transform) {}

    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, surface: &wl_surface::WlSurface, _: u32) {
        if let Some(win) = self.window.as_mut()
            && win.layer.wl_surface() == surface
        {
            win.frame_pending = false;
            self.redraw = true;
        }
    }

    fn surface_enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}

    fn surface_leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
}

impl OutputHandler for State {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl LayerShellHandler for State {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface) {
        if self.window.as_ref().is_some_and(|w| &w.layer == layer) {
            self.hide();
        }
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        if let Some(probe) = &self.probe
            && let Some(p) = probe.surfaces.iter().find(|p| &p.layer == layer)
        {
            let (w, h) = configure.new_size;
            p.viewport.set_destination(w.max(1) as i32, h.max(1) as i32);
            let surface = p.layer.wl_surface();
            surface.attach(Some(&p.buffer), 0, 0);
            surface.damage(0, 0, i32::MAX, i32::MAX);
            surface.commit();
            return;
        }
        let Some(win) = self.window.as_mut() else { return };
        if &win.layer != layer {
            return;
        }
        let (w, h) = configure.new_size;
        win.width = if w == 0 { 1920 } else { w };
        win.height = if h == 0 { 1080 } else { h };
        win.configured = true;
        win.needs_reconfigure = true;

        if win.surface.is_none() {
            let raw_display = RawDisplayHandle::Wayland(WaylandDisplayHandle::new(
                NonNull::new(self.conn.backend().display_ptr() as *mut _).expect("display"),
            ));
            let raw_window = RawWindowHandle::Wayland(WaylandWindowHandle::new(
                NonNull::new(win.layer.wl_surface().id().as_ptr() as *mut _).expect("surface"),
            ));
            let surface = unsafe {
                self.instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                    raw_display_handle: Some(raw_display),
                    raw_window_handle: raw_window,
                })
            };
            match surface {
                Ok(s) => {
                    if let Err(e) = self.ensure_gpu(&s) {
                        log::error!("{e:#}");
                        self.hide();
                        return;
                    }
                    if let Some(win) = self.window.as_mut() {
                        win.surface = Some(s);
                    }
                }
                Err(e) => {
                    log::error!("cannot create GPU surface: {e}");
                    self.hide();
                    return;
                }
            }
        }
        self.redraw = true;
    }
}

impl SeatHandler for State {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, seat: wl_seat::WlSeat) {
        self.seat.get_or_insert(seat);
    }

    fn new_capability(&mut self, _: &Connection, qh: &QueueHandle<Self>, seat: wl_seat::WlSeat, capability: Capability) {
        self.seat.get_or_insert(seat.clone());
        if capability == Capability::Keyboard && self.keyboard.is_none() {
            let lh = self.loop_handle.clone();
            match self.seat_state.get_keyboard_with_repeat(
                qh,
                &seat,
                None,
                lh,
                Box::new(|state: &mut State, _kbd, event| {
                    state.on_key(event, true, true);
                }),
            ) {
                Ok(k) => self.keyboard = Some(k),
                Err(e) => log::warn!("cannot get keyboard: {e}"),
            }
        }
        if capability == Capability::Pointer && self.pointer.is_none() {
            let cursor_surface = self.compositor.create_surface(qh);
            match self.seat_state.get_pointer_with_theme(
                qh,
                &seat,
                self.shm.wl_shm(),
                cursor_surface,
                ThemeSpec::default(),
            ) {
                Ok(p) => self.pointer = Some(p),
                Err(e) => log::warn!("cannot get pointer: {e}"),
            }
        }
    }

    fn remove_capability(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat, capability: Capability) {
        if capability == Capability::Keyboard
            && let Some(k) = self.keyboard.take()
        {
            k.release();
        }
        if capability == Capability::Pointer {
            self.pointer = None;
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl KeyboardHandler for State {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        surface: &wl_surface::WlSurface,
        serial: u32,
        _: &[u32],
        _: &[Keysym],
    ) {
        if self.is_our_surface(surface) {
            self.keyboard_focus = true;
            self.last_serial = serial;
        }
    }

    fn leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, surface: &wl_surface::WlSurface, _: u32) {
        if self.is_our_surface(surface) && self.keyboard_focus {
            self.keyboard_focus = false;
            if self.launcher.config.close_on_focus_out && self.pending_launch.is_none() {
                self.hide();
            }
        }
    }

    fn press_key(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, serial: u32, event: KeyEvent) {
        self.last_serial = serial;
        self.on_key(event, true, false);
    }

    fn repeat_key(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, event: KeyEvent) {
        self.on_key(event, true, true);
    }

    fn release_key(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, event: KeyEvent) {
        self.on_key(event, false, false);
    }

    fn update_modifiers(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        modifiers: Modifiers,
        _: RawModifiers,
        _: u32,
    ) {
        self.modifiers = modifiers;
    }
}

impl PointerHandler for State {
    fn pointer_frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_pointer::WlPointer, events: &[PointerEvent]) {
        let mods = egui_modifiers(&self.modifiers);
        if let Some(probe) = &self.probe {
            let hit = events.iter().find_map(|e| {
                if !matches!(e.kind, PointerEventKind::Enter { .. }) {
                    return None;
                }
                probe.surfaces.iter().find(|p| p.layer.wl_surface() == &e.surface).map(|p| p.output.clone())
            });
            if let Some(output) = hit {
                if let Some(p) = &self.pointer {
                    let _ = p.set_cursor(&self.conn, CursorIcon::Default);
                }
                self.finish_probe(Some(output));
            }
            return;
        }
        for event in events {
            if !self.is_our_surface(&event.surface) {
                continue;
            }
            let pos = egui::pos2(event.position.0 as f32, event.position.1 as f32);
            match event.kind {
                PointerEventKind::Enter { serial } => {
                    self.last_serial = serial;
                    self.pointer_pos = Some(pos);
                    self.egui_events.push(egui::Event::PointerMoved(pos));
                    if let Some(p) = &self.pointer {
                        let _ = p.set_cursor(&self.conn, CursorIcon::Default);
                    }
                }
                PointerEventKind::Leave { .. } => {
                    self.pointer_pos = None;
                    self.egui_events.push(egui::Event::PointerGone);
                }
                PointerEventKind::Motion { .. } => {
                    self.pointer_pos = Some(pos);
                    self.egui_events.push(egui::Event::PointerMoved(pos));
                }
                PointerEventKind::Press { button, serial, .. } | PointerEventKind::Release { button, serial, .. } => {
                    let pressed = matches!(event.kind, PointerEventKind::Press { .. });
                    if pressed {
                        self.last_serial = serial;
                    }
                    let button = match button {
                        0x110 => egui::PointerButton::Primary,
                        0x111 => egui::PointerButton::Secondary,
                        0x112 => egui::PointerButton::Middle,
                        _ => continue,
                    };
                    self.egui_events.push(egui::Event::PointerButton { pos, button, pressed, modifiers: mods });
                }
                PointerEventKind::Axis { horizontal, vertical, .. } => {
                    let dx = if horizontal.discrete != 0 { horizontal.discrete as f32 * 40.0 } else { horizontal.absolute as f32 };
                    let dy = if vertical.discrete != 0 { vertical.discrete as f32 * 40.0 } else { vertical.absolute as f32 };
                    self.egui_events.push(egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(-dx, -dy),
                        phase: egui::TouchPhase::Move,
                        source: egui::MouseWheelSource::Unknown,
                        modifiers: mods,
                    });
                }
            }
            self.redraw = true;
        }
    }
}

impl ActivationHandler for State {
    type RequestData = RequestData;

    fn new_token(&mut self, token: String, _data: &Self::RequestData) {
        if let Some(p) = self.pending_launch.take() {
            apps::spawn(&p.spec, Some(&token), &self.launcher.config.terminal);
            self.hide();
        }
    }
}

impl ShmHandler for State {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for State {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

impl Dispatch<WpFractionalScaleManagerV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &WpFractionalScaleManagerV1,
        _: <WpFractionalScaleManagerV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WpFractionalScaleV1, wl_surface::WlSurface> for State {
    fn event(
        state: &mut Self,
        _: &WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        surface: &wl_surface::WlSurface,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event
            && state.is_our_surface(surface)
        {
            state.set_scale(scale as f64 / 120.0, 1);
        }
    }
}

impl Dispatch<wl_buffer::WlBuffer, ()> for State {
    fn event(_: &mut Self, _: &wl_buffer::WlBuffer, _: wl_buffer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<KdeOutputOrderV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &KdeOutputOrderV1,
        event: kde_output_order_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            kde_output_order_v1::Event::Output { output_name } => state.output_order_pending.push(output_name),
            kde_output_order_v1::Event::Done => {
                state.output_order = std::mem::take(&mut state.output_order_pending);
                log::debug!("output order: {:?}", state.output_order);
            }
            _ => {}
        }
    }
}

impl Dispatch<WpViewporter, ()> for State {
    fn event(_: &mut Self, _: &WpViewporter, _: <WpViewporter as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<WpViewport, ()> for State {
    fn event(_: &mut Self, _: &WpViewport, _: <WpViewport as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

delegate_compositor!(State);
delegate_output!(State);
delegate_shm!(State);
delegate_seat!(State);
delegate_keyboard!(State);
delegate_pointer!(State);
delegate_layer!(State);
delegate_activation!(State);
delegate_registry!(State);
