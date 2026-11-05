//! Local LanguageTool protocol and exact UTF-16-to-Rust-comment mapping.
use rc_text::Rope;
use serde_json::Value;
use std::{
    ops::Range,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

#[derive(Debug, Clone)]
pub enum Error {
    Endpoint,
    Language,
    Limit,
    NoComments,
    Invalid,
    Network,
    Status(u16),
    Cancelled,
    Stale,
    Trust,
    Document,
}
pub struct Segment {
    pub text: String,
    pub doc_start: usize,
    pub utf16_start: usize,
    pub utf16_end: usize,
    excluded: Vec<Range<usize>>,
}
pub struct Input {
    pub text: String,
    pub segments: Vec<Segment>,
    source: Rope,
}
#[derive(Clone, Debug)]
pub struct Issue {
    pub range: Range<usize>,
    pub original: String,
    pub message: String,
    pub spelling: bool,
    pub replacements: Vec<String>,
    pub at_start: bool,
}

fn char_offset(text: &str, offset: usize) -> Option<usize> {
    let mut units = 0;
    for (index, ch) in text.chars().enumerate() {
        if units == offset {
            return Some(index);
        }
        units += ch.len_utf16();
        if units > offset {
            return None;
        }
    }
    (units == offset).then(|| text.chars().count())
}

pub fn prepare(rope: &Rope, cancel: &AtomicBool) -> Result<Input, Error> {
    if rope.len_bytes() > 1024 * 1024 {
        return Err(Error::Limit);
    }
    if cancel.load(Ordering::Acquire) {
        return Err(Error::Cancelled);
    }
    let tree = rc_syntax::parse_now(rc_syntax::Lang::Rust, rope, None).ok_or(Error::Document)?;
    if tree.root_node().has_error() {
        return Err(Error::Document);
    }
    let source = rope.to_string();
    let mut nodes = vec![tree.root_node()];
    let mut comments = Vec::new();
    let mut visited = 0;
    while let Some(node) = nodes.pop() {
        if cancel.load(Ordering::Acquire) {
            return Err(Error::Cancelled);
        }
        visited += 1;
        if visited > 100_000 {
            return Err(Error::Limit);
        }
        if matches!(node.kind(), "line_comment" | "block_comment") {
            comments.push(node.byte_range());
            continue;
        }
        let mut cursor = node.walk();
        nodes.extend(node.named_children(&mut cursor));
    }
    comments.sort_by_key(|r| r.start);
    let mut input = Input {
        text: String::new(),
        segments: Vec::new(),
        source: rope.clone(),
    };
    let mut units = 0;
    let mut fenced = false;
    let mut previous_line = None;
    for comment in comments {
        let raw = source.get(comment.clone()).ok_or(Error::Invalid)?;
        let block = raw.starts_with("/*");
        let mut byte = comment.start;
        for (index, line) in raw.split_inclusive('\n').enumerate() {
            let row = rope.byte_to_line(byte);
            if previous_line.is_some_and(|previous| row > previous + 1) {
                fenced = false;
            }
            previous_line = Some(row);
            let mut begin = line.len() - line.trim_start_matches([' ', '\t']).len();
            let mut end = line.trim_end_matches(['\r', '\n']).len();
            if index == 0 {
                let marker = if block { "/*" } else { "//" };
                if !line[begin..].starts_with(marker) {
                    return Err(Error::Invalid);
                }
                begin += 2;
                if matches!(line.as_bytes().get(begin), Some(b'!' | b'*' | b'/')) {
                    begin += 1;
                }
            } else if block && line.as_bytes().get(begin) == Some(&b'*') {
                begin += 1;
            }
            if block && line[..end].ends_with("*/") {
                end = end.saturating_sub(2);
            }
            let padding_start = begin;
            while begin < end && matches!(line.as_bytes()[begin], b' ' | b'\t') {
                begin += 1;
            }
            if begin - padding_start >= 4 || line[padding_start..begin].contains('\t') {
                byte += line.len();
                continue;
            }
            if begin >= end {
                byte += line.len();
                continue;
            }
            let piece = &line[begin..end];
            let trimmed = piece.trim();
            if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                fenced = !fenced;
                byte += line.len();
                continue;
            }
            if fenced || trimmed.is_empty() {
                byte += line.len();
                continue;
            }
            // Mixed fences and indented examples are code, not prose to rewrite.
            if piece.contains("```") || piece.contains("~~~") {
                byte += line.len();
                continue;
            }
            if input.segments.len() >= 128 {
                return Err(Error::Limit);
            }
            let length = piece.encode_utf16().count();
            if units + length + 2 > 16_384 {
                return Err(Error::Limit);
            }
            let mut excluded = Vec::new();
            let mut opened = None;
            for (at, ch) in piece.chars().enumerate() {
                if ch == '`' {
                    if let Some(start) = opened.take() {
                        excluded.push(start..at + 1);
                    } else {
                        opened = Some(at);
                    }
                }
            }
            if let Some(start) = opened {
                excluded.push(start..piece.chars().count());
            }
            input.segments.push(Segment {
                text: piece.into(),
                doc_start: rope.byte_to_char(byte + begin),
                utf16_start: units,
                utf16_end: units + length,
                excluded,
            });
            input.text.push_str(piece);
            input.text.push_str("\n\n");
            units += length + 2;
            byte += line.len();
        }
    }
    if input.segments.is_empty() {
        return Err(Error::NoComments);
    }
    Ok(input)
}

fn replacement(value: &str) -> bool {
    value.len() <= 1024
        && !value.contains(['\r', '\n', '\0'])
        && !value.contains("/*")
        && !value.contains("*/")
        && !value.contains('`')
        && !value.contains("~~~")
        && !value.chars().any(|c| {
            c.is_control()
                || matches!(c,'\u{2028}'|'\u{2029}'|'\u{202a}'..='\u{202e}'|'\u{2066}'..='\u{2069}')
        })
}

pub(crate) fn safe_in_source(
    source: &Rope,
    range: &Range<usize>,
    value: &str,
    first: bool,
) -> bool {
    if range.start > range.end || range.end > source.len_chars() || !replacement(value) {
        return false;
    }
    if first && (value.is_empty() || value.starts_with(char::is_whitespace)) {
        return false;
    }
    let before = range.start.checked_sub(1).map(|at| source.char(at));
    let after = (range.end < source.len_chars()).then(|| source.char(range.end));
    let original = source.slice(range.clone()).to_string();
    let context = |text: &str| {
        before
            .into_iter()
            .chain(text.chars())
            .chain(after)
            .collect::<String>()
    };
    for text in [context(&original), context(value)] {
        if text.contains("/*") || text.contains("*/") {
            return false;
        }
    }
    if range.start >= 2
        && source.slice(range.start - 2..range.start) == "//"
        && value.starts_with(['/', '!'])
    {
        return false;
    }
    true
}
pub fn decode(input: &Input, reply: &Value) -> Result<Vec<Issue>, Error> {
    let matches = reply
        .get("matches")
        .and_then(Value::as_array)
        .ok_or(Error::Invalid)?;
    if matches.len() > 256 {
        return Err(Error::Limit);
    }
    let mut issues = Vec::new();
    for item in matches {
        let offset = item
            .get("offset")
            .and_then(Value::as_u64)
            .and_then(|n| usize::try_from(n).ok())
            .ok_or(Error::Invalid)?;
        let length = item
            .get("length")
            .and_then(Value::as_u64)
            .and_then(|n| usize::try_from(n).ok())
            .ok_or(Error::Invalid)?;
        let end = offset.checked_add(length).ok_or(Error::Invalid)?;
        let segment = input
            .segments
            .iter()
            .find(|s| offset >= s.utf16_start && end <= s.utf16_end)
            .ok_or(Error::Invalid)?;
        let start =
            char_offset(&segment.text, offset - segment.utf16_start).ok_or(Error::Invalid)?;
        let end = char_offset(&segment.text, end - segment.utf16_start).ok_or(Error::Invalid)?;
        if segment
            .excluded
            .iter()
            .any(|r| start < r.end && end > r.start)
        {
            continue;
        }
        let original = segment.text.chars().skip(start).take(end - start).collect();
        let message = item
            .get("message")
            .and_then(Value::as_str)
            .ok_or(Error::Invalid)?;
        if message.len() > 4096 {
            return Err(Error::Limit);
        }
        let values = item
            .get("replacements")
            .and_then(Value::as_array)
            .ok_or(Error::Invalid)?;
        if values.len() > 100 {
            return Err(Error::Limit);
        }
        let mut replacements = Vec::new();
        for value in values.iter().take(8) {
            let value = value
                .get("value")
                .and_then(Value::as_str)
                .ok_or(Error::Invalid)?;
            let range = segment.doc_start + start..segment.doc_start + end;
            if !safe_in_source(&input.source, &range, value, start == 0) {
                continue;
            }
            replacements.push(value.into());
        }
        issues.push(Issue {
            range: segment.doc_start + start..segment.doc_start + end,
            original,
            message: message
                .chars()
                .take(512)
                .map(|c| if c.is_control() { ' ' } else { c })
                .collect(),
            spelling: item.pointer("/rule/issueType").and_then(Value::as_str)
                == Some("misspelling"),
            replacements,
            at_start: start == 0,
        });
    }
    Ok(issues)
}

pub async fn check(endpoint: &str, language: &str, input: &Input) -> Result<Vec<Issue>, Error> {
    let mut url = crate::local_ai_client::endpoint(endpoint).map_err(|_| Error::Endpoint)?;
    url.set_path("/v2/check");
    if language.is_empty()
        || language.len() > 24
        || !language
            .bytes()
            .all(|b| b.is_ascii_alphabetic() || b == b'-')
    {
        return Err(Error::Language);
    }
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|_| Error::Network)?;
    let mut form = reqwest::Url::parse("http://127.0.0.1/").map_err(|_| Error::Invalid)?;
    form.query_pairs_mut()
        .append_pair("language", language)
        .append_pair("text", &input.text);
    let mut response = client
        .post(url)
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .body(form.query().unwrap_or("").to_owned())
        .send()
        .await
        .map_err(|_| Error::Network)?;
    if response.content_length().is_some_and(|n| n > 1024 * 1024) {
        return Err(Error::Limit);
    }
    let status = response.status();
    if !status.is_success() {
        return Err(Error::Status(status.as_u16()));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| Error::Network)? {
        if bytes.len().saturating_add(chunk.len()) > 1024 * 1024 {
            return Err(Error::Limit);
        }
        bytes.extend_from_slice(&chunk);
    }
    let value = serde_json::from_slice(&bytes).map_err(|_| Error::Invalid)?;
    decode(input, &value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn rust_prose_mapping_skips_code_fences_and_rejects_comment_escape_or_surrogate_offsets() {
        let source = "/// 🦀 This is speling.\n/// ```rust\n/// let wrng = 1;\n/// ```\n/*\n * This is also speling.\n */\nfn main(){let code=\"speling\";}\n";
        let input = prepare(&Rope::from_str(source), &AtomicBool::new(false)).unwrap();
        assert_eq!(input.segments.len(), 2);
        assert!(!input.text.contains("wrng"));
        assert!(!input.text.contains("let code"));
        let word = input.text.find("speling").unwrap();
        let offset = input.text[..word].encode_utf16().count();
        let reply = json!({"matches":[{"offset":offset,"length":7,"message":"Spelling","rule":{"issueType":"misspelling"},"replacements":[{"value":"spelling"},{"value":"*/ panic!(); /*"},{"value":"new\ncode"}]}]});
        let issues = decode(&input, &reply).unwrap();
        assert_eq!(issues[0].replacements, vec!["spelling"]);
        assert_eq!(
            Rope::from_str(source)
                .slice(issues[0].range.clone())
                .to_string(),
            "speling"
        );
        let bad = json!({"matches":[{"offset":1,"length":1,"message":"Bad","replacements":[]}]});
        assert!(matches!(decode(&input, &bad), Err(Error::Invalid)));
        assert!(matches!(
            prepare(&Rope::from_str(source), &AtomicBool::new(true)),
            Err(Error::Cancelled)
        ));
    }

    #[test]
    fn corrections_cannot_create_comment_delimiters_or_turn_comment_prefixes_into_doc_directives() {
        let source = Rope::from_str("/* speling/ safely commented */\nfn main(){}\n");
        let input = prepare(&source, &AtomicBool::new(false)).unwrap();
        let reply = json!({"matches":[{"offset":0,"length":7,"message":"Bad boundary","replacements":[{"value":"spelling*"},{"value":"spelling"}]}]});
        let issues = decode(&input, &reply).unwrap();
        assert_eq!(issues[0].replacements, vec!["spelling"]);
        let source = Rope::from_str("//speling\nfn main(){}\n");
        assert!(!safe_in_source(&source, &(2..9), "/doc", true));
        assert!(!safe_in_source(&source, &(2..9), "!doc", true));
        assert!(!safe_in_source(&source, &(2..9), "", true));
        assert!(matches!(
            prepare(
                &Rope::from_str("///     let speling=1;\nfn main(){}\n"),
                &AtomicBool::new(false)
            ),
            Err(Error::NoComments)
        ));
        let input = prepare(
            &Rope::from_str("/// Write `speling` here.\nfn main(){}\n"),
            &AtomicBool::new(false),
        )
        .unwrap();
        let offset = input.text.find("speling").unwrap();
        let reply = json!({"matches":[{"offset":offset,"length":7,"message":"Code","replacements":[{"value":"spelling"}]}]});
        assert!(decode(&input, &reply).unwrap().is_empty());
    }

    #[test]
    #[ignore = "uses local test sockets"]
    fn transport_refuses_redirects_oversize_and_cancels_a_pending_request() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
        };
        fn request(stream: &mut std::net::TcpStream) {
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut bytes = Vec::new();
            let mut chunk = [0; 4096];
            loop {
                let n = stream.read(&mut chunk).unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&chunk[..n]);
                assert!(bytes.len() < 128 * 1024);
                if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&bytes[..end]);
                    let length = head
                        .lines()
                        .find_map(|line| {
                            line.to_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|n| n.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if bytes.len() >= end + 4 + length {
                        return;
                    }
                }
            }
        }
        for (reply, limit) in [
            (
                "HTTP/1.1 302 Found\r\nLocation: http://192.0.2.1/never\r\nContent-Length: 0\r\n\r\n",
                false,
            ),
            ("HTTP/1.1 200 OK\r\nContent-Length: 2097152\r\n\r\n", true),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let endpoint = format!("http://{}", listener.local_addr().unwrap());
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                request(&mut stream);
                stream.write_all(reply.as_bytes()).unwrap();
            });
            let input = prepare(
                &Rope::from_str("// A comment.\nfn main(){}\n"),
                &AtomicBool::new(false),
            )
            .unwrap();
            let result = rc_core::Runtime::block_on(check(&endpoint, "en-US", &input));
            if limit {
                assert!(matches!(result, Err(Error::Limit)));
            } else {
                assert!(matches!(result, Err(Error::Status(302))));
            }
            server.join().unwrap();
        }
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let (tx, rx) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            request(&mut stream);
            tx.send(()).unwrap();
            let mut byte = [0];
            match stream.read(&mut byte) {
                Ok(0) => (),
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::UnexpectedEof
                    ) =>
                {
                    ()
                }
                other => panic!("request did not close after cancellation: {other:?}"),
            }
        });
        let input = prepare(
            &Rope::from_str("// A comment.\nfn main(){}\n"),
            &AtomicBool::new(false),
        )
        .unwrap();
        let task = rc_core::Runtime::spawn(async move { check(&endpoint, "en-US", &input).await });
        rx.recv_timeout(Duration::from_secs(3)).unwrap();
        task.abort();
        assert!(rc_core::Runtime::block_on(task).unwrap_err().is_cancelled());
        server.join().unwrap();
        assert!(crate::local_ai_client::endpoint("https://example.com/").is_err());
    }
}
