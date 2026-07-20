//! `{{ }}` template + expression evaluation on embedded QuickJS.
//!
//! The sandbox invariant lives here: contexts receive NO host bindings — only
//! data globals injected as JSON. An expression can compute, never act. All
//! values cross the boundary as JSON text (string in, string out), which keeps
//! the engine seam small enough that a swap to another JS engine (boa) would
//! touch only this file.

use rquickjs::{Context, Runtime};
use serde_json::Value;
use std::time::{Duration, Instant};

/// Runtime-wide heap cap. One runtime exists per workflow run.
const MEMORY_LIMIT_BYTES: usize = 32 * 1024 * 1024;
/// Watchdog for a single `{{ }}` expression.
pub const EXPR_TIMEOUT: Duration = Duration::from_secs(1);
/// Default watchdog for a Code-node body (param-overridable at the node).
pub const CODE_TIMEOUT: Duration = Duration::from_secs(5);

/// One JS runtime per workflow run.
pub struct ExprEngine {
    runtime: Runtime,
}

impl ExprEngine {
    pub fn new() -> Result<Self, String> {
        let runtime = Runtime::new().map_err(|e| format!("js runtime: {e}"))?;
        runtime.set_memory_limit(MEMORY_LIMIT_BYTES);
        Ok(ExprEngine { runtime })
    }

    /// A fresh context for one node execution. Fresh per node so one node's
    /// expressions cannot pollute another's globals; within a node the scope is
    /// shared across per-item evaluations (deliberate, cheap).
    pub fn scope(&self) -> Result<ExprScope<'_>, String> {
        let ctx = Context::full(&self.runtime).map_err(|e| format!("js context: {e}"))?;
        Ok(ExprScope { engine: self, ctx })
    }
}

pub struct ExprScope<'a> {
    engine: &'a ExprEngine,
    ctx: Context,
}

impl ExprScope<'_> {
    /// Define a global as parsed JSON data.
    pub fn set_global(&self, name: &str, value: &Value) -> Result<(), String> {
        // Trailing `undefined` so the program's completion value is convertible
        // regardless of what the assignment evaluated to.
        let src = format!(
            "globalThis.{name} = JSON.parse({});\nundefined",
            js_string_literal(value)
        );
        self.raw_eval_string(&src, Duration::from_secs(5)).map(|_| ())
    }

    /// Run arbitrary setup statements (used for the `$node` / `$secret` helper
    /// functions, whose backing data is injected as JSON first).
    pub fn set_prelude(&self, statements: &str) -> Result<(), String> {
        let src = format!("{statements}\nundefined");
        self.raw_eval_string(&src, Duration::from_secs(5)).map(|_| ())
    }

    /// Evaluate one `{{ }}` expression to a JSON value. `undefined` → null.
    /// JS exceptions come back as Err with the thrown message.
    pub fn eval_expr(&self, expr: &str, timeout: Duration) -> Result<Value, String> {
        // Envelope: always yields a JSON string; thrown errors are caught JS-side
        // so we can surface `e.message` instead of an opaque host error.
        let src = format!(
            "(function() {{ try {{ var __r = JSON.stringify({{ ok: (function() {{ return ({expr}\n); }})() }});\n\
             return __r === undefined ? '{{}}' : __r; }} catch (e) {{\n\
             return JSON.stringify({{ err: String(e && e.message !== undefined ? e.message : e) }}); }} }})()"
        );
        let raw = self.raw_eval_string(&src, timeout)?;
        let envelope: Value =
            serde_json::from_str(&raw).map_err(|e| format!("expression produced unserializable data: {e}"))?;
        if let Some(err) = envelope.get("err").and_then(|v| v.as_str()) {
            return Err(err.to_string());
        }
        // Missing "ok" key = the expression evaluated to undefined (or something
        // JSON cannot carry, like a function) — normalize to null.
        Ok(envelope.get("ok").cloned().unwrap_or(Value::Null))
    }

    /// Evaluate a Code-node body. `body` is the user's statements; they run
    /// inside a function receiving the declared arguments (already injected as
    /// globals by the caller). Returns whatever the body `return`s, as JSON.
    pub fn eval_code(&self, wrapped: &str, timeout: Duration) -> Result<Value, String> {
        self.eval_expr(wrapped, timeout)
    }

    /// The one place JS actually executes: interrupt watchdog armed, string out.
    fn raw_eval_string(&self, src: &str, timeout: Duration) -> Result<String, String> {
        let deadline = Instant::now() + timeout;
        self.engine
            .runtime
            .set_interrupt_handler(Some(Box::new(move || Instant::now() >= deadline)));
        let result = self.ctx.with(|ctx| {
            match ctx.eval::<Option<String>, _>(src.as_bytes()) {
                Ok(Some(s)) => Ok(s),
                Ok(None) => Ok("null".to_string()),
                Err(rquickjs::Error::Exception) => {
                    let caught = ctx.catch();
                    let msg = caught
                        .as_object()
                        .and_then(|o| o.get::<_, String>("message").ok())
                        .or_else(|| caught.as_string().and_then(|s| s.to_string().ok()))
                        .unwrap_or_else(|| "javascript exception".to_string());
                    Err(msg)
                }
                Err(e) => Err(e.to_string()),
            }
        });
        self.engine.runtime.set_interrupt_handler(None);
        result.map_err(|e| {
            if Instant::now() >= deadline {
                format!("expression timed out after {}ms", timeout.as_millis())
            } else {
                e
            }
        })
    }
}

/// Embed a JSON value as a JS string literal (for `JSON.parse(<literal>)`).
/// serde's string escaping is JS-compatible; U+2028/U+2029 are escaped for
/// belt-and-braces even though modern engines accept them in strings.
pub fn js_string_literal(v: &Value) -> String {
    let json_text = serde_json::to_string(v).unwrap_or_else(|_| "null".into());
    let lit = serde_json::to_string(&json_text).unwrap_or_else(|_| "\"null\"".into());
    lit.replace('\u{2028}', "\\u2028").replace('\u{2029}', "\\u2029")
}

// --------------------------------------------------------------- templates

/// Does this string contain a `{{ }}` template?
pub fn is_template(s: &str) -> bool {
    s.contains("{{")
}

/// Resolve every `{{ expr }}` in `text`. If the entire string is exactly one
/// expression, the result keeps its JSON type; otherwise segment results are
/// stringified (strings verbatim, everything else as JSON) and concatenated.
pub fn resolve_template(scope: &ExprScope<'_>, text: &str) -> Result<Value, String> {
    let segments = split_template(text)?;
    // Whole-string single expression → typed passthrough.
    if segments.len() == 1 {
        if let Segment::Expr(e) = &segments[0] {
            return scope.eval_expr(e, EXPR_TIMEOUT);
        }
    }
    let mut out = String::new();
    for seg in &segments {
        match seg {
            Segment::Text(t) => out.push_str(t),
            Segment::Expr(e) => {
                let v = scope.eval_expr(e, EXPR_TIMEOUT)?;
                match v {
                    Value::String(s) => out.push_str(&s),
                    Value::Null => {}
                    other => out.push_str(&other.to_string()),
                }
            }
        }
    }
    Ok(Value::String(out))
}

/// Walk a params tree resolving templates in every string leaf.
/// Applies to node params ONLY — item data is never resolved (the injection
/// boundary; see design doc §6.2).
pub fn resolve_params(scope: &ExprScope<'_>, params: &Value) -> Result<Value, String> {
    Ok(match params {
        Value::String(s) if is_template(s) => resolve_template(scope, s)?,
        Value::Array(a) => Value::Array(a.iter().map(|v| resolve_params(scope, v)).collect::<Result<_, _>>()?),
        Value::Object(o) => {
            let mut out = serde_json::Map::with_capacity(o.len());
            for (k, v) in o {
                out.insert(k.clone(), resolve_params(scope, v)?);
            }
            Value::Object(out)
        }
        other => other.clone(),
    })
}

enum Segment {
    Text(String),
    Expr(String),
}

/// Split on `{{ … }}`, non-nested, first `}}` closes. Unclosed `{{` is an error.
fn split_template(text: &str) -> Result<Vec<Segment>, String> {
    let mut segments = Vec::new();
    let mut rest = text;
    loop {
        match rest.find("{{") {
            None => {
                if !rest.is_empty() {
                    segments.push(Segment::Text(rest.to_string()));
                }
                break;
            }
            Some(start) => {
                if start > 0 {
                    segments.push(Segment::Text(rest[..start].to_string()));
                }
                let after = &rest[start + 2..];
                let end = after
                    .find("}}")
                    .ok_or_else(|| "unclosed {{ in template".to_string())?;
                segments.push(Segment::Expr(after[..end].trim().to_string()));
                rest = &after[end + 2..];
            }
        }
    }
    if segments.is_empty() {
        segments.push(Segment::Text(String::new()));
    }
    Ok(segments)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn engine() -> ExprEngine {
        ExprEngine::new().expect("runtime")
    }

    #[test]
    fn eval_arithmetic_and_types() {
        let e = engine();
        let s = e.scope().unwrap();
        assert_eq!(s.eval_expr("1 + 2", EXPR_TIMEOUT).unwrap(), json!(3));
        assert_eq!(s.eval_expr("'a' + 'b'", EXPR_TIMEOUT).unwrap(), json!("ab"));
        assert_eq!(s.eval_expr("({x: [1,2]})", EXPR_TIMEOUT).unwrap(), json!({"x": [1,2]}));
        assert_eq!(s.eval_expr("undefined", EXPR_TIMEOUT).unwrap(), Value::Null);
    }

    #[test]
    fn globals_round_trip() {
        let e = engine();
        let s = e.scope().unwrap();
        s.set_global("$json", &json!({"name": "Swerve", "n": 2})).unwrap();
        assert_eq!(
            s.eval_expr("$json.name.toUpperCase() + $json.n", EXPR_TIMEOUT).unwrap(),
            json!("SWERVE2")
        );
    }

    #[test]
    fn exceptions_surface_their_message() {
        let e = engine();
        let s = e.scope().unwrap();
        let err = s.eval_expr("nope.deep", EXPR_TIMEOUT).unwrap_err();
        assert!(err.contains("nope"), "got: {err}");
    }

    #[test]
    fn infinite_loop_is_interrupted() {
        let e = engine();
        let s = e.scope().unwrap();
        let started = Instant::now();
        let err = s
            .eval_expr("(function(){ while(true){} })()", Duration::from_millis(200))
            .unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(5), "watchdog too slow");
        assert!(err.contains("timed out") || err.to_lowercase().contains("interrupt"), "got: {err}");
    }

    #[test]
    fn memory_limit_is_enforced() {
        let e = engine();
        let s = e.scope().unwrap();
        let err = s.eval_expr(
            "(function(){ var a = []; while(true) { a.push(new Array(65536).fill('x')); } })()",
            Duration::from_secs(10),
        );
        assert!(err.is_err(), "allocation bomb must fail");
    }

    #[test]
    fn no_host_bindings_exist() {
        let e = engine();
        let s = e.scope().unwrap();
        for probe in ["typeof fetch", "typeof require", "typeof process", "typeof os", "typeof std"] {
            assert_eq!(
                s.eval_expr(probe, EXPR_TIMEOUT).unwrap(),
                json!("undefined"),
                "sandbox leak: {probe}"
            );
        }
    }

    #[test]
    fn whole_string_single_expression_keeps_type() {
        let e = engine();
        let s = e.scope().unwrap();
        s.set_global("$json", &json!({"n": 7})).unwrap();
        assert_eq!(resolve_template(&s, "{{ $json.n }}").unwrap(), json!(7));
        assert_eq!(resolve_template(&s, "n = {{ $json.n }}!").unwrap(), json!("n = 7!"));
    }

    #[test]
    fn literal_braces_escape_hatch() {
        let e = engine();
        let s = e.scope().unwrap();
        assert_eq!(resolve_template(&s, "{{ \"{{\" }} literal").unwrap(), json!("{{ literal"));
    }

    #[test]
    fn params_resolve_recursively_and_leave_non_templates_alone() {
        let e = engine();
        let s = e.scope().unwrap();
        s.set_global("$json", &json!({"host": "example.com"})).unwrap();
        let params = json!({
            "url": "https://{{ $json.host }}/api",
            "count": 3,
            "nested": { "plain": "no braces", "typed": "{{ 1 + 1 }}" },
            "list": ["{{ 'a' }}", "b"]
        });
        let resolved = resolve_params(&s, &params).unwrap();
        assert_eq!(
            resolved,
            json!({
                "url": "https://example.com/api",
                "count": 3,
                "nested": { "plain": "no braces", "typed": 2 },
                "list": ["a", "b"]
            })
        );
    }

    #[test]
    fn unclosed_template_is_an_error() {
        let e = engine();
        let s = e.scope().unwrap();
        assert!(resolve_template(&s, "broken {{ 1 + ").is_err());
    }
}
