//! Tiny JSON: enough for the handful of control-plane message shapes. No serde.
use std::fmt;

mod parse;
pub use parse::parse;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Value>),
    Obj(Vec<(String, Value)>),
}

impl Value {
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Obj(kv) => kv.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Num(n) => Some(*n),
            _ => None,
        }
    }
    pub fn as_i64(&self) -> Option<i64> {
        self.as_f64().map(|n| n as i64)
    }
    /// Field as a string: strings as-is, numbers and bools rendered. Config values arrive both ways.
    pub fn field_string(&self, key: &str) -> Option<String> {
        match self.get(key)? {
            Value::Str(s) => Some(s.clone()),
            Value::Null => None,
            v => Some(v.to_string()),
        }
    }
    pub fn obj() -> Value {
        Value::Obj(Vec::new())
    }
    pub fn set(mut self, key: &str, v: impl Into<Value>) -> Value {
        if let Value::Obj(kv) = &mut self {
            kv.push((key.to_string(), v.into()));
        }
        self
    }
    /// Append all fields of another object.
    pub fn extend(mut self, other: Value) -> Value {
        if let (Value::Obj(kv), Value::Obj(more)) = (&mut self, other) {
            kv.extend(more);
        }
        self
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::Str(s.to_string())
    }
}
impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::Str(s)
    }
}
impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Value::Bool(b)
    }
}
impl From<f64> for Value {
    fn from(n: f64) -> Self {
        Value::Num(n)
    }
}
impl From<u64> for Value {
    fn from(n: u64) -> Self {
        Value::Num(n as f64)
    }
}
impl From<i64> for Value {
    fn from(n: i64) -> Self {
        Value::Num(n as f64)
    }
}
impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(o: Option<T>) -> Self {
        o.map(Into::into).unwrap_or(Value::Null)
    }
}
impl From<Vec<Value>> for Value {
    fn from(v: Vec<Value>) -> Self {
        Value::Arr(v)
    }
}

fn write_str(f: &mut fmt::Formatter<'_>, s: &str) -> fmt::Result {
    f.write_str("\"")?;
    for c in s.chars() {
        match c {
            '"' => f.write_str("\\\"")?,
            '\\' => f.write_str("\\\\")?,
            '\n' => f.write_str("\\n")?,
            '\r' => f.write_str("\\r")?,
            '\t' => f.write_str("\\t")?,
            c if (c as u32) < 0x20 => write!(f, "\\u{:04x}", c as u32)?,
            c => write!(f, "{c}")?,
        }
    }
    f.write_str("\"")
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Null => f.write_str("null"),
            Value::Bool(b) => write!(f, "{b}"),
            // NaN and the infinities have no JSON form ("NaN" would break the reader).
            Value::Num(n) if !n.is_finite() => f.write_str("null"),
            Value::Num(n) if n.fract() == 0.0 && n.abs() < 1e15 => write!(f, "{}", *n as i64),
            Value::Num(n) => write!(f, "{n}"),
            Value::Str(s) => write_str(f, s),
            Value::Arr(a) => {
                f.write_str("[")?;
                for (i, v) in a.iter().enumerate() {
                    if i > 0 {
                        f.write_str(",")?;
                    }
                    write!(f, "{v}")?;
                }
                f.write_str("]")
            }
            Value::Obj(kv) => {
                f.write_str("{")?;
                for (i, (k, v)) in kv.iter().enumerate() {
                    if i > 0 {
                        f.write_str(",")?;
                    }
                    write_str(f, k)?;
                    write!(f, ":{v}")?;
                }
                f.write_str("}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_message_shapes() {
        let txt = r#"{"type":"config","url":"http://nas:8080/frame/k","rotation":3,
            "brightness":80,"screen_off":"23:00","screen_on":null,"page_version":"7","ok":true,
            "list":[1,2.5,-3e2],"nested":{"a":[]}}"#;
        let v = parse(txt).unwrap();
        assert_eq!(v.get("type").and_then(Value::as_str), Some("config"));
        assert_eq!(v.get("rotation").and_then(Value::as_i64), Some(3));
        assert_eq!(v.field_string("brightness"), Some("80".into()));
        assert_eq!(v.field_string("screen_on"), None);
        assert_eq!(v.get("ok"), Some(&Value::Bool(true)));
        assert_eq!(
            v.get("list"),
            Some(&Value::Arr(vec![
                Value::Num(1.0),
                Value::Num(2.5),
                Value::Num(-300.0)
            ]))
        );
        let again = parse(&v.to_string()).unwrap();
        assert_eq!(v, again);
    }

    #[test]
    fn escapes_both_ways() {
        let v = parse(r#""a\"b\\c\n\u00e9\ud83d\ude00""#).unwrap();
        assert_eq!(v.as_str(), Some("a\"b\\c\n\u{e9}\u{1F600}"));
        assert_eq!(v.to_string(), "\"a\\\"b\\\\c\\n\u{e9}\u{1F600}\"");
        assert_eq!(Value::from("\u{1}").to_string(), "\"\\u0001\"");
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse("{").is_err());
        assert!(parse("[1,]").is_err());
        assert!(parse("tru").is_err());
        assert!(parse("{} x").is_err());
        assert!(parse("\"abc").is_err());
    }

    /// Found by fuzz.rs: `"\uDBFF\u0000"` overflowed u32 in the surrogate arithmetic (a panic
    /// in debug, a wrong char in release).
    #[test]
    fn unpaired_high_surrogate_is_replaced_not_combined() {
        assert_eq!(
            parse(r#""\uDBFF\u0000x""#).unwrap().as_str(),
            Some("\u{FFFD}\u{0}x")
        );
        assert_eq!(parse(r#""\uD800A""#).unwrap().as_str(), Some("\u{FFFD}A"));
        assert_eq!(parse(r#""\uD800""#).unwrap().as_str(), Some("\u{FFFD}"));
        assert_eq!(parse(r#""􏿿""#).unwrap().as_str(), Some("\u{10FFFF}"));
    }

    /// Deep `[[[[` recursed once per level until the stack overflowed (an abort, not an Err).
    #[test]
    fn nesting_is_limited() {
        let ok = format!(
            "{}{}",
            "[".repeat(parse::MAX_DEPTH),
            "]".repeat(parse::MAX_DEPTH)
        );
        assert!(parse(&ok).is_ok());
        let deep = format!("{}1{}", "[".repeat(65), "]".repeat(65));
        assert!(parse(&deep).unwrap_err().contains("nested deeper"));
        assert!(parse(&"[{\"a\":".repeat(200_000)).is_err());
    }

    /// `1e999` parsed to infinity and was written back as `inf`, which no JSON reader takes.
    #[test]
    fn non_finite_numbers_are_refused_and_never_written() {
        assert!(parse("1e999").is_err());
        assert!(parse("[-1e400]").is_err());
        assert_eq!(Value::Num(f64::NAN).to_string(), "null");
        assert_eq!(Value::Num(f64::INFINITY).to_string(), "null");
    }

    #[test]
    fn builder_and_numbers() {
        let v = Value::obj()
            .set("type", "ack")
            .set("id", 42u64)
            .set("ok", true)
            .set("err", None::<String>)
            .set("t", 1.5);
        assert_eq!(
            v.to_string(),
            r#"{"type":"ack","id":42,"ok":true,"err":null,"t":1.5}"#
        );
    }
}
