//! A tiny calculator for queries like `2 * (3 + 4)` or `sqrt(2)^2`.

pub fn looks_like_math(q: &str) -> bool {
    let q = q.trim();
    if q.is_empty() || !q.chars().any(|c| c.is_ascii_digit()) {
        return false;
    }
    let allowed = |c: char| c.is_ascii_alphanumeric() || " .,+-*/%^()_".contains(c);
    if !q.chars().all(allowed) {
        return false;
    }
    // Needs an operator or a function call, so plain numbers / app names stay app searches.
    q.chars().any(|c| "+-*/%^(".contains(c))
}

pub fn eval(q: &str) -> Option<f64> {
    let tokens = tokenize(q)?;
    let mut p = Parser { tokens, pos: 0 };
    let v = p.expr()?;
    if p.pos != p.tokens.len() {
        return None;
    }
    v.is_finite().then_some(v)
}

pub fn format_number(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    if v.abs() >= 1e15 || v.abs() < 1e-9 {
        return format!("{v:e}");
    }
    let s = format!("{v:.10}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Ident(String),
    Op(char),
}

fn tokenize(s: &str) -> Option<Vec<Tok>> {
    let mut out = Vec::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c.is_ascii_digit() || c == '.' || c == ',' {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.' || chars[i] == ',') {
                i += 1;
            }
            let num: String = chars[start..i].iter().map(|&c| if c == ',' { '.' } else { c }).collect();
            out.push(Tok::Num(num.parse().ok()?));
        } else if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            out.push(Tok::Ident(chars[start..i].iter().collect::<String>().to_lowercase()));
        } else if "+-*/%^()".contains(c) {
            // `**` is accepted as power too.
            if c == '*' && chars.get(i + 1) == Some(&'*') {
                out.push(Tok::Op('^'));
                i += 2;
            } else {
                out.push(Tok::Op(c));
                i += 1;
            }
        } else {
            return None;
        }
    }
    Some(out)
}

struct Parser {
    tokens: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.tokens.get(self.pos)
    }

    fn eat_op(&mut self, op: char) -> bool {
        if self.peek() == Some(&Tok::Op(op)) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expr(&mut self) -> Option<f64> {
        let mut v = self.term()?;
        loop {
            if self.eat_op('+') {
                v += self.term()?;
            } else if self.eat_op('-') {
                v -= self.term()?;
            } else {
                return Some(v);
            }
        }
    }

    fn term(&mut self) -> Option<f64> {
        let mut v = self.unary()?;
        loop {
            if self.eat_op('*') {
                v *= self.unary()?;
            } else if self.eat_op('/') {
                v /= self.unary()?;
            } else if self.eat_op('%') {
                v %= self.unary()?;
            } else {
                return Some(v);
            }
        }
    }

    fn unary(&mut self) -> Option<f64> {
        if self.eat_op('-') {
            return Some(-self.unary()?);
        }
        if self.eat_op('+') {
            return self.unary();
        }
        self.power()
    }

    fn power(&mut self) -> Option<f64> {
        let base = self.primary()?;
        if self.eat_op('^') {
            // right associative, and binds tighter than unary minus on the left
            let exp = self.unary()?;
            return Some(base.powf(exp));
        }
        Some(base)
    }

    fn primary(&mut self) -> Option<f64> {
        match self.tokens.get(self.pos)?.clone() {
            Tok::Num(n) => {
                self.pos += 1;
                Some(n)
            }
            Tok::Op('(') => {
                self.pos += 1;
                let v = self.expr()?;
                self.eat_op(')').then_some(v)
            }
            Tok::Ident(name) => {
                self.pos += 1;
                match name.as_str() {
                    "pi" => return Some(std::f64::consts::PI),
                    "e" => return Some(std::f64::consts::E),
                    "tau" => return Some(std::f64::consts::TAU),
                    _ => {}
                }
                if !self.eat_op('(') {
                    return None;
                }
                let arg = self.expr()?;
                if !self.eat_op(')') {
                    return None;
                }
                Some(match name.as_str() {
                    "sqrt" => arg.sqrt(),
                    "abs" => arg.abs(),
                    "exp" => arg.exp(),
                    "ln" => arg.ln(),
                    "log" | "log10" => arg.log10(),
                    "log2" => arg.log2(),
                    "sin" => arg.sin(),
                    "cos" => arg.cos(),
                    "tan" => arg.tan(),
                    "asin" => arg.asin(),
                    "acos" => arg.acos(),
                    "atan" => arg.atan(),
                    "sinh" => arg.sinh(),
                    "cosh" => arg.cosh(),
                    "tanh" => arg.tanh(),
                    "floor" => arg.floor(),
                    "ceil" => arg.ceil(),
                    "round" => arg.round(),
                    _ => return None,
                })
            }
            Tok::Op(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluates() {
        assert_eq!(eval("2 * (3 + 4)"), Some(14.0));
        assert_eq!(eval("2^3^2"), Some(512.0));
        assert_eq!(eval("-2^2"), Some(-4.0));
        assert_eq!(eval("10 % 3"), Some(1.0));
        assert_eq!(eval("1,5 * 2"), Some(3.0));
        assert_eq!(eval("sqrt(16) + 1"), Some(5.0));
        assert_eq!(eval("2 +"), None);
        assert_eq!(format_number(0.1 + 0.2), "0.3");
        assert!(looks_like_math("1+1"));
        assert!(!looks_like_math("firefox"));
        assert!(!looks_like_math("2048"));
    }
}
