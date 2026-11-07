//! Tiny expression language for parametric values.
//!
//! Supports numbers with optional units (`mm`, `cm`, `m`, `in`, `ft`, `deg`, `rad`),
//! `+ - * / ^`, parentheses, unary minus, named parameters and a few functions
//! (`sin cos tan asin acos atan sqrt abs min max floor ceil round`) plus `pi`.
//!
//! Internal units: lengths in millimetres, angles in degrees. Trig functions take
//! and return degrees, matching what a CAD user types.

use std::collections::HashMap;
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub struct ExprError(pub String);

impl fmt::Display for ExprError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for ExprError {}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Ident(String),
    Op(char),
    LParen,
    RParen,
    Comma,
}

fn tokenize(s: &str) -> Result<Vec<Tok>, ExprError> {
    let mut out = Vec::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c.is_ascii_digit() || c == '.' {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            // exponent
            if i < chars.len() && (chars[i] == 'e' || chars[i] == 'E') {
                let save = i;
                i += 1;
                if i < chars.len() && (chars[i] == '+' || chars[i] == '-') {
                    i += 1;
                }
                if i < chars.len() && chars[i].is_ascii_digit() {
                    while i < chars.len() && chars[i].is_ascii_digit() {
                        i += 1;
                    }
                } else {
                    i = save;
                }
            }
            let txt: String = chars[start..i].iter().collect();
            let v: f64 = txt
                .parse()
                .map_err(|_| ExprError(format!("bad number '{txt}'")))?;
            out.push(Tok::Num(v));
        } else if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            out.push(Tok::Ident(chars[start..i].iter().collect()));
        } else {
            match c {
                '+' | '-' | '*' | '/' | '^' => out.push(Tok::Op(c)),
                '(' => out.push(Tok::LParen),
                ')' => out.push(Tok::RParen),
                ',' => out.push(Tok::Comma),
                _ => return Err(ExprError(format!("unexpected character '{c}'"))),
            }
            i += 1;
        }
    }
    Ok(out)
}

fn unit_factor(name: &str) -> Option<f64> {
    Some(match name {
        "mm" => 1.0,
        "cm" => 10.0,
        "m" => 1000.0,
        "in" => 25.4,
        "ft" => 304.8,
        "deg" => 1.0,
        "rad" => 180.0 / std::f64::consts::PI,
        _ => return None,
    })
}

struct Parser<'a> {
    toks: Vec<Tok>,
    pos: usize,
    vars: &'a dyn Fn(&str) -> Option<f64>,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }
    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        self.pos += 1;
        t
    }

    fn expr(&mut self) -> Result<f64, ExprError> {
        let mut v = self.term()?;
        while let Some(Tok::Op(c @ ('+' | '-'))) = self.peek().cloned() {
            self.pos += 1;
            let r = self.term()?;
            v = if c == '+' { v + r } else { v - r };
        }
        Ok(v)
    }

    fn term(&mut self) -> Result<f64, ExprError> {
        let mut v = self.unary()?;
        while let Some(Tok::Op(c @ ('*' | '/'))) = self.peek().cloned() {
            self.pos += 1;
            let r = self.unary()?;
            v = if c == '*' {
                v * r
            } else {
                if r == 0.0 {
                    return Err(ExprError("division by zero".into()));
                }
                v / r
            };
        }
        Ok(v)
    }

    fn unary(&mut self) -> Result<f64, ExprError> {
        match self.peek() {
            Some(Tok::Op('-')) => {
                self.pos += 1;
                Ok(-self.unary()?)
            }
            Some(Tok::Op('+')) => {
                self.pos += 1;
                self.unary()
            }
            _ => self.power(),
        }
    }

    fn power(&mut self) -> Result<f64, ExprError> {
        let base = self.atom()?;
        if let Some(Tok::Op('^')) = self.peek() {
            self.pos += 1;
            let e = self.unary()?; // right associative
            return Ok(base.powf(e));
        }
        Ok(base)
    }

    fn atom(&mut self) -> Result<f64, ExprError> {
        match self.next() {
            Some(Tok::Num(v)) => {
                // optional unit suffix
                if let Some(Tok::Ident(u)) = self.peek().cloned() {
                    if let Some(f) = unit_factor(&u) {
                        self.pos += 1;
                        return Ok(v * f);
                    }
                }
                Ok(v)
            }
            Some(Tok::LParen) => {
                let v = self.expr()?;
                match self.next() {
                    Some(Tok::RParen) => Ok(v),
                    _ => Err(ExprError("missing ')'".into())),
                }
            }
            Some(Tok::Ident(name)) => {
                if let Some(Tok::LParen) = self.peek() {
                    self.pos += 1;
                    let mut args = Vec::new();
                    if !matches!(self.peek(), Some(Tok::RParen)) {
                        loop {
                            args.push(self.expr()?);
                            match self.next() {
                                Some(Tok::Comma) => continue,
                                Some(Tok::RParen) => break,
                                _ => return Err(ExprError("expected ',' or ')'".into())),
                            }
                        }
                    } else {
                        self.pos += 1;
                    }
                    return call(&name, &args);
                }
                if name == "pi" {
                    return Ok(std::f64::consts::PI);
                }
                if let Some(v) = (self.vars)(&name) {
                    return Ok(v);
                }
                Err(ExprError(format!("unknown parameter '{name}'")))
            }
            Some(t) => Err(ExprError(format!("unexpected token {t:?}"))),
            None => Err(ExprError("unexpected end of expression".into())),
        }
    }
}

fn call(name: &str, a: &[f64]) -> Result<f64, ExprError> {
    let d2r = std::f64::consts::PI / 180.0;
    let need = |n: usize| -> Result<(), ExprError> {
        if a.len() == n {
            Ok(())
        } else {
            Err(ExprError(format!("{name}() expects {n} argument(s)")))
        }
    };
    Ok(match name {
        "sin" => { need(1)?; (a[0] * d2r).sin() }
        "cos" => { need(1)?; (a[0] * d2r).cos() }
        "tan" => { need(1)?; (a[0] * d2r).tan() }
        "asin" => { need(1)?; a[0].asin() / d2r }
        "acos" => { need(1)?; a[0].acos() / d2r }
        "atan" => { need(1)?; a[0].atan() / d2r }
        "sqrt" => { need(1)?; a[0].sqrt() }
        "abs" => { need(1)?; a[0].abs() }
        "floor" => { need(1)?; a[0].floor() }
        "ceil" => { need(1)?; a[0].ceil() }
        "round" => { need(1)?; a[0].round() }
        "min" => { need(2)?; a[0].min(a[1]) }
        "max" => { need(2)?; a[0].max(a[1]) }
        _ => return Err(ExprError(format!("unknown function '{name}'"))),
    })
}

/// Evaluate `src` with a variable lookup callback.
pub fn eval_with(src: &str, vars: &dyn Fn(&str) -> Option<f64>) -> Result<f64, ExprError> {
    let toks = tokenize(src)?;
    if toks.is_empty() {
        return Err(ExprError("empty expression".into()));
    }
    let mut p = Parser { toks, pos: 0, vars };
    let v = p.expr()?;
    if p.pos != p.toks.len() {
        return Err(ExprError("unexpected trailing input".into()));
    }
    if !v.is_finite() {
        return Err(ExprError("result is not a finite number".into()));
    }
    Ok(v)
}

/// Evaluate `src` against a map of already-resolved parameter values.
pub fn eval(src: &str, vars: &HashMap<String, f64>) -> Result<f64, ExprError> {
    eval_with(src, &|n| vars.get(n).copied())
}

/// Identifiers referenced by an expression (used for dependency ordering).
pub fn identifiers(src: &str) -> Vec<String> {
    tokenize(src)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|t| match t {
            Tok::Ident(s) if unit_factor(&s).is_none() && s != "pi" => Some(s),
            _ => None,
        })
        .collect()
}

/// Resolve a set of named expressions that may reference each other.
/// Returns resolved values and per-name errors (cycles, unknown names, ...).
pub fn resolve_parameters(
    params: &[(String, String)],
) -> (HashMap<String, f64>, HashMap<String, String>) {
    let mut values = HashMap::new();
    let mut errors = HashMap::new();
    let mut pending: Vec<&(String, String)> = params.iter().collect();
    loop {
        let before = pending.len();
        pending.retain(|(name, ex)| match eval(ex, &values) {
            Ok(v) => {
                values.insert(name.clone(), v);
                false
            }
            Err(_) => true,
        });
        if pending.is_empty() || pending.len() == before {
            break;
        }
    }
    for (name, ex) in pending {
        let msg = match eval(ex, &values) {
            Err(e) => e.0,
            Ok(_) => "circular reference".into(),
        };
        errors.insert(name.clone(), msg);
    }
    (values, errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arithmetic_and_units() {
        let m = HashMap::new();
        assert_eq!(eval("1 + 2 * 3", &m).unwrap(), 7.0);
        assert_eq!(eval("(1 + 2) * 3", &m).unwrap(), 9.0);
        assert_eq!(eval("-2^2", &m).unwrap(), -4.0);
        assert_eq!(eval("2 cm + 5 mm", &m).unwrap(), 25.0);
        assert_eq!(eval("1 in", &m).unwrap(), 25.4);
        assert!((eval("sin(30)", &m).unwrap() - 0.5).abs() < 1e-12);
        assert_eq!(eval("max(3, 7)", &m).unwrap(), 7.0);
        assert!(eval("1 +", &m).is_err());
        assert!(eval("foo", &m).is_err());
    }

    #[test]
    fn parameters_resolve_in_any_order() {
        let p = vec![
            ("height".to_string(), "width / 2".to_string()),
            ("width".to_string(), "40 mm".to_string()),
            ("a".to_string(), "b".to_string()),
            ("b".to_string(), "a".to_string()),
        ];
        let (v, e) = resolve_parameters(&p);
        assert_eq!(v["height"], 20.0);
        assert!(e.contains_key("a") && e.contains_key("b"));
    }
}
