//! The VEP/1 protocol: framing and frames (INTERNALS ch. 9). Pure: no threads, no interpreter;
//! sessions (`engines/session`) drive it, the codec (`codec/vep`) fills the values.

#![allow(dead_code)] // sessions (roadmap 1.3) are the first callers

use std::io::{BufRead, BufReader, Read, Write};

use serde_json::{json, Map, Value as Json};

/// The largest frame either side may send: 64 MiB (9.1).
pub const MAX_FRAME: usize = 64 << 20;

/// Bracket nesting a frame may have before it is parsed (10.1). It only bounds serde_json's
/// recursion; the codec enforces the 256 value levels of 10.5, which `dict` and `error` tags
/// spread over more brackets.
const MAX_FRAME_NESTING: usize = 1000;

#[derive(Debug)]
pub enum ReadErr {
    TooLong,
    Io(std::io::Error),
}

/// Reads frames: raw bytes split on `\n` only, one `\r` before it dropped (9.1).
pub struct FrameReader<R> {
    r: BufReader<R>,
}

impl<R: Read> FrameReader<R> {
    pub fn new(r: R) -> FrameReader<R> {
        FrameReader { r: BufReader::with_capacity(64 * 1024, r) }
    }

    /// The next line, or `None` at end of input (a partial last line counts as the end: the
    /// worker died mid-frame). A line past 64 MiB is refused as soon as it gets there.
    pub fn read_line(&mut self) -> Result<Option<Vec<u8>>, ReadErr> {
        let mut line = Vec::new();
        loop {
            let buf = match self.r.fill_buf() {
                Ok(b) => b,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(ReadErr::Io(e)),
            };
            if buf.is_empty() {
                return Ok(None);
            }
            let (chunk, done) = match buf.iter().position(|&b| b == b'\n') {
                Some(i) => (&buf[..i], i + 1),
                None => (buf, buf.len()),
            };
            if line.len() + chunk.len() > MAX_FRAME {
                return Err(ReadErr::TooLong);
            }
            line.extend_from_slice(chunk);
            let found = done > chunk.len();
            self.r.consume(done);
            if found {
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                return Ok(Some(line));
            }
        }
    }
}

#[derive(Debug)]
pub enum WriteErr {
    /// The frame's size in bytes; nothing was sent (9.1).
    TooBig(usize),
    Io(std::io::Error),
}

/// One compact frame plus `\n`, in one `write_all`, then a flush (8.5, 9.1).
pub fn write_frame(w: &mut dyn Write, frame: &Json) -> Result<(), WriteErr> {
    let mut bytes = serde_json::to_vec(frame).map_err(|e| WriteErr::Io(e.into()))?;
    if bytes.len() > MAX_FRAME {
        return Err(WriteErr::TooBig(bytes.len()));
    }
    bytes.push(b'\n');
    w.write_all(&bytes).and_then(|_| w.flush()).map_err(WriteErr::Io)
}

/// `MarshalError: value too large to send to engine 'py' (70 MiB, limit 64 MiB)` (9.1).
pub fn too_big_message(tag: &str, size: usize) -> String {
    format!("value too large to send to engine '{tag}' ({} MiB, limit 64 MiB)", size.div_ceil(1 << 20))
}

/// `engine 'py' broke the protocol: <what> (got: <first 80 bytes, escaped>)` (9.11).
pub fn protocol_error(tag: &str, what: &str, got: &[u8]) -> String {
    let head = &got[..got.len().min(80)];
    let mut shown = String::new();
    for c in String::from_utf8_lossy(head).chars() {
        match c {
            '\n' => shown.push_str("\\n"),
            '\r' => shown.push_str("\\r"),
            '\t' => shown.push_str("\\t"),
            '\\' => shown.push_str("\\\\"),
            c if c.is_control() => shown.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => shown.push(c),
        }
    }
    if got.len() > 80 {
        shown.push('…');
    }
    format!("engine '{tag}' broke the protocol: {what} (got: {shown})")
}

/// The capabilities a worker announced in `hello` (9.3, 17.5).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Caps {
    pub modules: bool,
    pub handles: bool,
    pub kwargs: bool,
    pub reverse: bool,
}

/// `hello` is `{t, vep, version, pid, caps}` (9.3); other members are ignored.
#[derive(Clone, Debug, PartialEq)]
pub struct Hello {
    pub version: String,
    pub pid: u32,
    pub caps: Caps,
}

/// A frame from a worker (9.2).
#[derive(Clone, Debug, PartialEq)]
pub enum Frame {
    Ret { id: u64, value: Json, exports: Option<Json>, missing: Option<Json> },
    Err { id: u64, error: Map<String, Json> },
    Call { id: u64, target: Target, args: Vec<Json>, kwargs: Map<String, Json> },
    Out { stream: Stream, text: String },
    Release { fns: Vec<u64> },
}

/// What a worker's `call` calls (9.5): a route of an inner `@import`, or a V2 callable.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Target {
    Route(u64),
    Fn(u64),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Stream {
    Stdout,
    Stderr,
}

/// Bracket nesting outside strings, cut off just past `limit`.
fn nesting(line: &[u8], limit: usize) -> usize {
    let (mut depth, mut max, mut in_str, mut esc) = (0usize, 0usize, false, false);
    for &b in line {
        if in_str {
            match b {
                _ if esc => esc = false,
                b'\\' => esc = true,
                b'"' => in_str = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'[' | b'{' => {
                depth += 1;
                max = max.max(depth);
                if max > limit {
                    return max;
                }
            }
            b']' | b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    max
}

/// A line as a JSON object, or what is wrong with it.
fn object(line: &[u8]) -> Result<Map<String, Json>, String> {
    if nesting(line, MAX_FRAME_NESTING) > MAX_FRAME_NESTING {
        return Err(format!("JSON nested deeper than {MAX_FRAME_NESTING} levels"));
    }
    let mut de = serde_json::Deserializer::from_slice(line);
    de.disable_recursion_limit();
    let mut values = de.into_iter::<Json>();
    let v = match values.next() {
        Some(Ok(v)) => v,
        Some(Err(e)) => return Err(format!("not valid JSON ({e})")),
        None => return Err("an empty line".into()),
    };
    let end = values.byte_offset();
    if line[end..].iter().any(|b| !matches!(b, b' ' | b'\t' | b'\r' | b'\n')) {
        return Err("not valid JSON (trailing characters)".into());
    }
    match v {
        Json::Object(m) => Ok(m),
        _ => Err("a line that is not a JSON object".into()),
    }
}

/// Before `hello`, a line's preload noise for stderr and the frame after it (9.3). Noise with no
/// newline of its own shares `hello`'s line.
pub fn split_noise(line: &[u8]) -> (&[u8], &[u8]) {
    const HELLO: &[u8] = br#"{"t":"hello""#;
    match line.windows(HELLO.len()).position(|w| w == HELLO) {
        Some(i) => line.split_at(i),
        None if object(line).is_err() => (line, &[]),
        None => (&[], line),
    }
}

fn member<'a>(m: &'a Map<String, Json>, t: &str, name: &str) -> Result<&'a Json, String> {
    m.get(name).ok_or_else(|| format!("'{t}' frame without '{name}'"))
}

fn uint(m: &Map<String, Json>, t: &str, name: &str) -> Result<u64, String> {
    member(m, t, name)?.as_u64().ok_or_else(|| format!("'{t}' frame with a bad '{name}'"))
}

fn text<'a>(m: &'a Map<String, Json>, t: &str, name: &str) -> Result<&'a str, String> {
    member(m, t, name)?.as_str().ok_or_else(|| format!("'{t}' frame with a bad '{name}'"))
}

/// The first frame: `hello` with `vep: 1` (9.3).
pub fn parse_hello(line: &[u8]) -> Result<Hello, String> {
    let m = object(line)?;
    if m.get("t").and_then(Json::as_str) != Some("hello") {
        return Err("the first frame is not 'hello'".into());
    }
    let vep = uint(&m, "hello", "vep")?;
    if vep != 1 {
        return Err(format!("speaks VEP/{vep}; this V2 speaks VEP/1"));
    }
    let caps_list = member(&m, "hello", "caps")?.as_array().ok_or("'hello' frame with a bad 'caps'")?;
    let mut caps = Caps::default();
    for c in caps_list {
        match c.as_str() {
            Some("modules") => caps.modules = true,
            Some("handles") => caps.handles = true,
            Some("kwargs") => caps.kwargs = true,
            Some("reverse") => caps.reverse = true,
            Some(_) => {} // unknown caps are ignored (9.3)
            None => return Err("'hello' frame with a bad 'caps'".into()),
        }
    }
    let pid = u32::try_from(uint(&m, "hello", "pid")?).map_err(|_| "'hello' frame with a bad 'pid'")?;
    Ok(Hello { version: text(&m, "hello", "version")?.to_string(), pid, caps })
}

/// A frame after `hello`, checked against 9.11's fatal cases.
pub fn parse_frame(line: &[u8], caps: Caps) -> Result<Frame, String> {
    let m = object(line)?;
    let t = m.get("t").and_then(Json::as_str).ok_or("a frame without a string 't'")?.to_string();
    match t.as_str() {
        "ret" => Ok(Frame::Ret {
            id: uint(&m, &t, "id")?,
            value: member(&m, &t, "value")?.clone(),
            exports: m.get("exports").cloned(),
            missing: m.get("missing").cloned(),
        }),
        "err" => {
            let error = member(&m, &t, "error")?.as_object().ok_or("'err' frame with a bad 'error'")?;
            Ok(Frame::Err { id: uint(&m, &t, "id")?, error: error.clone() })
        }
        "call" => {
            if !caps.reverse {
                return Err("a 'call' from a worker without the 'reverse' capability".into());
            }
            let target = member(&m, &t, "target")?.as_object().ok_or("'call' frame with a bad 'target'")?;
            if target.contains_key("unit") || target.contains_key("h") {
                return Err("a 'call' to a unit or handle target, which only the host may send".into());
            }
            let target = match (target.get("route").and_then(Json::as_u64), target.get("fn").and_then(Json::as_u64)) {
                (Some(r), None) => Target::Route(r),
                (None, Some(f)) => Target::Fn(f),
                _ => return Err("'call' frame with a bad 'target'".into()),
            };
            let args = member(&m, &t, "args")?.as_array().ok_or("'call' frame with a bad 'args'")?.clone();
            let kwargs = member(&m, &t, "kwargs")?.as_object().ok_or("'call' frame with a bad 'kwargs'")?.clone();
            Ok(Frame::Call { id: uint(&m, &t, "id")?, target, args, kwargs })
        }
        "out" => {
            let stream = match text(&m, &t, "stream")? {
                "stdout" => Stream::Stdout,
                "stderr" => Stream::Stderr,
                _ => return Err("'out' frame with a bad 'stream'".into()),
            };
            Ok(Frame::Out { stream, text: text(&m, &t, "text")?.to_string() })
        }
        "release" => {
            let list = member(&m, &t, "fn")?.as_array().ok_or("'release' frame with a bad 'fn'")?;
            let fns = list.iter().map(Json::as_u64).collect::<Option<Vec<u64>>>().ok_or("'release' frame with a bad 'fn'")?;
            Ok(Frame::Release { fns })
        }
        "hello" | "init" | "load" | "module" | "op" | "bye" => Err(format!("a '{t}' frame, which a worker may not send")),
        other => Err(format!("an unknown frame type '{other}'")),
    }
}

// ---------------------------------------------------------------------------------------------
// Host → worker frames (9.2)

/// `init`: the project root (9.3).
pub fn init(id: u64, project_root: &str) -> Json {
    json!({"t": "init", "id": id, "project_root": project_root})
}

/// What a `load` frame carries (9.4, 12.2); `request` gives it its id.
pub struct Load<'a> {
    pub unit: &'a str,
    pub definition: bool,
    pub lang: &'a str,
    pub file: &'a str,
    pub line: u32,
    pub source: &'a str,
    pub args: Map<String, Json>,
    pub imports: Vec<Json>,
    pub exports: Vec<Json>,
    pub want_value: bool,
}

pub fn load(l: Load) -> Json {
    json!({
        "t": "load", "unit": l.unit, "kind": if l.definition { "definition" } else { "action" }, "lang": l.lang,
        "file": l.file, "line": l.line, "source": l.source, "args": l.args, "imports": l.imports,
        "exports": l.exports, "want_value": l.want_value,
    })
}

pub fn ret(id: u64, value: Json) -> Json {
    json!({"t": "ret", "id": id, "value": value})
}

pub fn err(id: u64, error: Json) -> Json {
    json!({"t": "err", "id": id, "error": error})
}

/// Handle ids the host dropped, once per reference (9.10).
pub fn release(ids: &[u64]) -> Json {
    json!({"t": "release", "h": ids})
}

pub fn bye() -> Json {
    json!({"t": "bye"})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(input: &[u8]) -> Vec<Result<Option<Vec<u8>>, String>> {
        let mut r = FrameReader::new(input);
        let mut out = Vec::new();
        loop {
            match r.read_line() {
                Ok(Some(l)) => out.push(Ok(Some(l))),
                Ok(None) => {
                    out.push(Ok(None));
                    return out;
                }
                Err(e) => {
                    out.push(Err(format!("{e:?}")));
                    return out;
                }
            }
        }
    }

    #[test]
    fn splits_on_lf_only() {
        // \r, U+0085, U+2028 and \x1c inside a line are not line ends.
        let input = "{\"text\":\"a\rb\u{85}c\u{2028}d\x1ce\"}\n{\"t\":2}\n".as_bytes();
        let got = lines(input);
        assert_eq!(got.len(), 3);
        assert_eq!(got[0].as_ref().unwrap().as_deref(), Some("{\"text\":\"a\rb\u{85}c\u{2028}d\x1ce\"}".as_bytes()));
        assert_eq!(got[1].as_ref().unwrap().as_deref(), Some(&b"{\"t\":2}"[..]));
        assert_eq!(got[2].as_ref().unwrap(), &None);
    }

    #[test]
    fn drops_one_cr() {
        let got = lines(b"a\r\r\nb\r\n");
        assert_eq!(got[0].as_ref().unwrap().as_deref(), Some(&b"a\r"[..]));
        assert_eq!(got[1].as_ref().unwrap().as_deref(), Some(&b"b"[..]));
        // A partial last line is the end of input: the worker died mid-frame.
        assert_eq!(lines(b"{\"t\":"), [Ok(None)]);
    }

    #[test]
    fn frames_over_64_mib_are_refused_both_ways() {
        let mut big = vec![b'x'; MAX_FRAME + 1];
        big.push(b'\n');
        let mut r = FrameReader::new(&big[..]);
        assert!(matches!(r.read_line(), Err(ReadErr::TooLong)));
        let mut ok = vec![b'y'; MAX_FRAME];
        ok.push(b'\n');
        assert_eq!(FrameReader::new(&ok[..]).read_line().unwrap().unwrap().len(), MAX_FRAME);
        let mut sink = Vec::new();
        let frame = json!({"t": "ret", "id": 1, "value": "z".repeat(MAX_FRAME)});
        match write_frame(&mut sink, &frame) {
            Err(WriteErr::TooBig(n)) => assert!(n > MAX_FRAME),
            other => panic!("{other:?}"),
        }
        assert!(sink.is_empty(), "nothing is sent");
        assert_eq!(too_big_message("py", 70 << 20), "value too large to send to engine 'py' (70 MiB, limit 64 MiB)");
        write_frame(&mut sink, &bye()).unwrap();
        assert_eq!(sink, b"{\"t\":\"bye\"}\n");
    }

    #[test]
    fn hello_and_noise() {
        assert_eq!(split_noise(b"site customization loaded"), (&b"site customization loaded"[..], &b""[..]));
        assert_eq!(split_noise(b"[1, 2]"), (&b"[1, 2]"[..], &b""[..]));
        assert_eq!(split_noise(br#"{"t":"hello"}"#), (&b""[..], &br#"{"t":"hello"}"#[..]));
        assert_eq!(split_noise(br#"{"t":"ret"}"#), (&b""[..], &br#"{"t":"ret"}"#[..]));
        assert_eq!(split_noise(br#"x{"t":"hello"}"#), (&b"x"[..], &br#"{"t":"hello"}"#[..]));
        let h = parse_hello(br#"{"t":"hello","vep":1,"version":"3.12.4","pid":4242,"caps":["modules","handles","kwargs","reverse","future"],"extra":1}"#).unwrap();
        assert_eq!((h.version.as_str(), h.pid, h.caps), ("3.12.4", 4242, Caps { modules: true, handles: true, kwargs: true, reverse: true }));
        // `engine` and `impl` are not part of `hello` (9.3): neither is needed, both are ignored.
        let h = parse_hello(br#"{"t":"hello","vep":1,"engine":"py","impl":"CPython","version":"3.12.4","pid":1,"caps":[]}"#).unwrap();
        assert_eq!(h, Hello { version: "3.12.4".into(), pid: 1, caps: Caps::default() });
        assert_eq!(parse_hello(br#"{"t":"hello","vep":2,"version":"1","pid":1,"caps":[]}"#), Err("speaks VEP/2; this V2 speaks VEP/1".into()));
        assert_eq!(parse_hello(br#"{"t":"hello","vep":1,"pid":1,"caps":[]}"#), Err("'hello' frame without 'version'".into()));
        assert_eq!(parse_hello(br#"{"t":"ret","id":1,"value":null}"#), Err("the first frame is not 'hello'".into()));
        assert_eq!(parse_hello(br#"{"t":"hello","vep":1}"#), Err("'hello' frame without 'caps'".into()));
    }

    #[test]
    fn worker_forbidden_frames_and_targets() {
        let all = Caps { modules: true, handles: true, kwargs: true, reverse: true };
        for t in ["hello", "init", "load", "module", "op", "bye"] {
            let line = format!("{{\"t\":\"{t}\",\"id\":1}}");
            assert_eq!(parse_frame(line.as_bytes(), all), Err(format!("a '{t}' frame, which a worker may not send")));
        }
        let call = |target: &str| format!(r#"{{"t":"call","id":1,"target":{target},"args":[],"kwargs":{{}}}}"#);
        assert_eq!(parse_frame(call(r#"{"fn":3}"#).as_bytes(), all).unwrap(), Frame::Call { id: 1, target: Target::Fn(3), args: vec![], kwargs: Map::new() });
        assert!(matches!(parse_frame(call(r#"{"route":2}"#).as_bytes(), all).unwrap(), Frame::Call { target: Target::Route(2), .. }));
        assert!(parse_frame(call(r#"{"unit":"0:1","name":"f"}"#).as_bytes(), all).unwrap_err().contains("only the host may send"));
        assert!(parse_frame(call(r#"{"h":1}"#).as_bytes(), all).unwrap_err().contains("only the host may send"));
        assert!(parse_frame(call(r#"{"fn":3}"#).as_bytes(), Caps::default()).unwrap_err().contains("'reverse' capability"));
        assert!(parse_frame(call(r#"{"fn":3,"route":1}"#).as_bytes(), all).unwrap_err().contains("bad 'target'"));
    }

    #[test]
    fn unknown_t_and_missing_members_are_fatal() {
        let c = Caps::default();
        assert_eq!(parse_frame(br#"{"t":"nope"}"#, c), Err("an unknown frame type 'nope'".into()));
        assert_eq!(parse_frame(br#"{"id":1}"#, c), Err("a frame without a string 't'".into()));
        assert_eq!(parse_frame(br#"{"t":"ret","id":1}"#, c), Err("'ret' frame without 'value'".into()));
        assert_eq!(parse_frame(br#"{"t":"ret","id":-1,"value":1}"#, c), Err("'ret' frame with a bad 'id'".into()));
        assert_eq!(parse_frame(br#"{"t":"out","stream":"log","text":"x"}"#, c), Err("'out' frame with a bad 'stream'".into()));
        assert_eq!(parse_frame(br#"{"t":"release","fn":[1,"2"]}"#, c), Err("'release' frame with a bad 'fn'".into()));
        assert_eq!(parse_frame(b"[1]", c), Err("a line that is not a JSON object".into()));
        assert!(parse_frame(b"{\"t\":\"out\",\"stream\":\"stdout\",\"text\":\"\xff\"}", c).unwrap_err().starts_with("not valid JSON"));
        assert!(parse_frame(br#"{"t":"out","stream":"stdout","text":"\ud800"}"#, c).unwrap_err().starts_with("not valid JSON"));
        // The byte scan only bounds the parser (10.1); the codec enforces 256 value levels.
        let deep = format!("{{\"t\":\"ret\",\"id\":1,\"value\":{}{}}}", "[".repeat(1000), "]".repeat(1000));
        assert_eq!(parse_frame(deep.as_bytes(), c), Err("JSON nested deeper than 1000 levels".into()));
        let scanned = format!("{{\"t\":\"ret\",\"id\":1,\"value\":{}{}}}", "[".repeat(998), "]".repeat(998));
        // Frames are parsed on the interpreter thread and its 256 MiB stack (1.3).
        let parsed = std::thread::Builder::new().stack_size(64 << 20).spawn(move || parse_frame(scanned.as_bytes(), c).is_ok());
        assert!(parsed.unwrap().join().unwrap());
        let ok = format!("{{\"t\":\"ret\",\"id\":1,\"value\":{}{}}}", "[".repeat(256), "]".repeat(256));
        assert!(parse_frame(ok.as_bytes(), c).is_ok());
        assert_eq!(
            parse_frame(br#"{"t":"out","stream":"stderr","text":"x","new":true}"#, c),
            Ok(Frame::Out { stream: Stream::Stderr, text: "x".into() }),
            "unknown members are ignored"
        );
    }

    #[test]
    fn protocol_error_text() {
        assert_eq!(
            protocol_error("py", "an unknown frame type 'x'", b"{\"t\":\"x\"}\r\n\x01"),
            "engine 'py' broke the protocol: an unknown frame type 'x' (got: {\"t\":\"x\"}\\r\\n\\u{1})"
        );
        let long = protocol_error("js", "w", &[b'a'; 200]);
        assert!(long.ends_with(&format!("(got: {}…)", "a".repeat(80))), "{long}");
    }

    #[test]
    fn brackets_inside_strings_do_not_count() {
        let c = Caps::default();
        for raw in [format!("\\\"{}", "[".repeat(300)), format!("{}\\\\", "{".repeat(300))] {
            let line = format!("{{\"t\":\"out\",\"stream\":\"stdout\",\"text\":\"{raw}\"}}");
            let text: String = serde_json::from_str(&format!("\"{raw}\"")).unwrap();
            assert_eq!(parse_frame(line.as_bytes(), c), Ok(Frame::Out { stream: Stream::Stdout, text }), "{raw}");
        }
    }

    #[test]
    fn trailing_characters_are_fatal() {
        assert_eq!(
            parse_frame(br#"{"t":"out","stream":"stdout","text":"a"} {"t":"bye"}"#, Caps::default()),
            Err("not valid JSON (trailing characters)".into())
        );
    }

    #[test]
    fn exactly_64_mib_goes_out() {
        let overhead = serde_json::to_vec(&json!({"t": "ret", "id": 1, "value": ""})).unwrap().len();
        let mut sink = Vec::new();
        write_frame(&mut sink, &json!({"t": "ret", "id": 1, "value": "z".repeat(MAX_FRAME - overhead)})).expect("exactly the limit is sent");
        assert_eq!(sink.len(), MAX_FRAME + 1);
        assert_eq!(too_big_message("py", MAX_FRAME + 1), "value too large to send to engine 'py' (65 MiB, limit 64 MiB)");
    }
}
