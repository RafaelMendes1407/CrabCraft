//! HTTP request files (`.http`, `.rest`, F-03): the requests they hold and the role of each
//! line, for highlighting and for the gutter's run action.
//!
//! The format is the plain-text one shared by several editors: requests are separated by lines
//! starting with `###`; each has comment lines (`#` or `//`, with `@name` naming the request),
//! a request line (`METHOD URL [HTTP/version]`, or just a URL for GET), optional query lines
//! that continue the URL, headers up to the first blank line and then the body.
//!
//! File-wide variables (`{{name}}`, `@name = value`) are expanded by the client through
//! [`vars::Resolver`]. Unresolved references, file bodies, response scripts and response
//! redirects are reported as [`Unsupported`]. A bounded inline handler is retained for
//! explicit review; this parser never evaluates an environment or script.

use crate::highlight::{Hl, Span};
#[path = "http_vars.rs"]
pub mod vars;

/// The most lines looked back for the start of the request a line belongs to.
pub(crate) const MAX_LOOK_BACK: usize = 2_000;

/// What a line of an HTTP file is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineRole {
    /// `###`, optionally followed by the request's name.
    Separator,
    /// `# …` or `// …` before the request line or among the headers.
    Comment,
    /// `@name = value` (a file-wide variable declaration).
    FileVariable,
    /// `METHOD URL [HTTP/x]`.
    Request,
    /// An indented `?…` or `&…` line that continues the URL.
    UrlContinuation,
    Header,
    Blank,
    Body,
    /// `< ./file`: the body comes from a file (not supported yet).
    BodyFile,
    /// `> {% … %}` or `> ./script.js`: a response handler (not supported yet).
    Handler,
    /// `>> ./file`: saves the response to a file (not supported yet).
    Redirect,
}

/// Something in a request that CrabCraft does not do yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnsupportedKind {
    /// `{{name}}`.
    Variable(String),
    /// `@name = value`.
    FileVariable(String),
    /// `< ./file`.
    BodyFile,
    /// `> {% … %}` or `> script.js`.
    Handler,
    /// `>> file`.
    Redirect,
    /// `WEBSOCKET`, `GRPC`, `GRAPHQL`…
    Method(String),
    /// `# @no-redirect`, `# @timeout`…: other request directives.
    Directive(String),
}

impl UnsupportedKind {
    /// Whether sending the request anyway would send something other than what was written.
    pub fn blocks_sending(&self) -> bool {
        matches!(self, UnsupportedKind::Variable(_) | UnsupportedKind::BodyFile | UnsupportedKind::Method(_))
    }
}

/// Where an unsupported construct is, as char offsets in the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unsupported {
    pub kind: UnsupportedKind,
    pub start: usize,
    pub end: usize,
}

/// One request of an HTTP file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HttpRequest {
    /// From `### name` or `# @name name`.
    pub name: Option<String>,
    /// The request line (0-based).
    pub line: usize,
    pub method: String,
    /// As written, with continuation lines joined.
    pub url: String,
    /// `HTTP/1.1`, `HTTP/2`… when written.
    pub version: Option<String>,
    pub headers: Vec<(String, String)>,
    pub body: String,
    pub unsupported: Vec<Unsupported>,
    /// One complete inline response handler, retained for explicit review only.
    pub response_script: Option<String>,
    /// File-wide declarations before request lines; shared across all request blocks.
    pub variables: std::sync::Arc<std::collections::BTreeMap<String, String>>,
}

impl HttpRequest {
    /// The name, or `METHOD URL`.
    pub fn title(&self) -> String {
        match &self.name {
            Some(n) => n.clone(),
            None => format!("{} {}", self.method, self.url),
        }
    }

    /// The constructs that keep the request from being sent.
    pub fn blockers(&self) -> impl Iterator<Item = &Unsupported> {
        let mut resolver = vars::Resolver::new(&self.variables);
        self.unsupported.iter().filter(move |u| match &u.kind {
            UnsupportedKind::Variable(name) => resolver.variable(name).is_err(),
            kind => kind.blocks_sending(),
        })
    }
}

/// The methods a request line may start with.
const METHODS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS", "TRACE", "CONNECT"];
/// Request kinds of other clients that are not plain HTTP.
const OTHER_METHODS: &[&str] = &["WEBSOCKET", "GRPC", "GRAPHQL", "SSE"];

fn is_separator(line: &str) -> bool {
    line.starts_with("###")
}

fn is_comment(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with('#') || t.starts_with("//")
}

/// `@name = value` at the start of a line.
fn file_variable(line: &str) -> Option<&str> {
    let rest = line.strip_prefix('@')?;
    let (name, _) = rest.split_once('=')?;
    let name = name.trim();
    (!name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'))).then_some(name)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    /// Before the request line.
    Start,
    /// After the request line, before the blank line.
    Headers {
        any_header: bool,
    },
    Body {
        first: bool,
    },
    /// Inside a multi-line `> {% … %}`.
    Script,
}

/// Roles of consecutive lines, the first one starting a request block (after a `###` or at the
/// top of the file).
pub fn line_roles<'a>(lines: impl IntoIterator<Item = &'a str>) -> Vec<LineRole> {
    let mut state = State::Start;
    let mut out = Vec::new();
    for raw in lines {
        let line = raw.trim_end_matches(['\n', '\r']);
        let blank = line.trim().is_empty();
        if is_separator(line) && state != State::Script {
            state = State::Start;
            out.push(LineRole::Separator);
            continue;
        }
        let role = match state {
            State::Start if blank => LineRole::Blank,
            State::Start if is_comment(line) => LineRole::Comment,
            State::Start if file_variable(line).is_some() => LineRole::FileVariable,
            State::Start => {
                state = State::Headers { any_header: false };
                LineRole::Request
            }
            State::Headers { .. } if blank => {
                state = State::Body { first: true };
                LineRole::Blank
            }
            State::Headers { any_header } => {
                let t = line.trim_start();
                if !any_header && line.starts_with([' ', '\t']) && (t.starts_with('?') || t.starts_with('&')) {
                    LineRole::UrlContinuation
                } else if is_comment(line) {
                    LineRole::Comment
                } else {
                    state = State::Headers { any_header: true };
                    LineRole::Header
                }
            }
            State::Body { first } => {
                state = State::Body { first: false };
                if line.starts_with(">>") {
                    LineRole::Redirect
                } else if line.starts_with("> ") || line == ">" {
                    if line.contains("{%") && !line.contains("%}") {
                        state = State::Script;
                    }
                    LineRole::Handler
                } else if first && line.starts_with("< ") {
                    LineRole::BodyFile
                } else if blank {
                    LineRole::Blank
                } else {
                    LineRole::Body
                }
            }
            State::Script => {
                if line.contains("%}") {
                    state = State::Body { first: false };
                }
                LineRole::Handler
            }
        };
        out.push(role);
    }
    out
}

/// The requests of an HTTP file.
pub fn parse_requests(text: &str) -> Vec<HttpRequest> {
    parse_blocks(text).into_iter().filter_map(|(req, found)| found.then_some(req)).collect()
}

/// Everything of the file that is not supported yet, also outside requests (a file variable
/// above the first one).
pub fn unsupported_in(text: &str) -> Vec<Unsupported> {
    parse_blocks(text)
        .into_iter()
        .flat_map(|(req, _)| {
            let mut resolver = vars::Resolver::new(&req.variables);
            req.unsupported
                .into_iter()
                .filter(|item| match &item.kind {
                    UnsupportedKind::FileVariable(_) => false,
                    UnsupportedKind::Variable(name) => resolver.variable(name).is_err(),
                    _ => true,
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Each block between separators, and whether it has a request line.
fn parse_blocks(text: &str) -> Vec<(HttpRequest, bool)> {
    let lines: Vec<&str> = text.split('\n').collect();
    let roles = line_roles(lines.iter().copied());
    let variables: std::sync::Arc<std::collections::BTreeMap<String, String>> = std::sync::Arc::new(
        lines
            .iter()
            .zip(&roles)
            .filter(|(_, role)| **role == LineRole::FileVariable)
            .filter_map(|(line, _)| {
                let name = file_variable(line)?;
                let (_, value) = line.split_once('=')?;
                Some((name.to_string(), value.trim().to_string()))
            })
            .collect(),
    );
    // Char offset of each line's start.
    let mut starts = Vec::with_capacity(lines.len());
    let mut at = 0;
    for l in &lines {
        starts.push(at);
        at += l.chars().count() + 1;
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        // One block: from here to the next separator.
        let mut name = None;
        if roles[i] == LineRole::Separator {
            let title = lines[i].trim_start_matches('#').trim();
            if !title.is_empty() {
                name = Some(title.to_string());
            }
            i += 1;
        }
        let begin = i;
        while i < lines.len() && roles[i] != LineRole::Separator {
            i += 1;
        }
        let (mut request, found) = parse_block(&lines[begin..i], &roles[begin..i], &starts[begin..i], begin, name);
        request.variables = variables.clone();
        out.push((request, found));
    }
    out
}

fn parse_block(lines: &[&str], roles: &[LineRole], starts: &[usize], first_line: usize, mut name: Option<String>) -> (HttpRequest, bool) {
    let mut req = HttpRequest::default();
    let mut body: Vec<&str> = Vec::new();
    let mut found = false;
    for (k, (raw, role)) in lines.iter().zip(roles).enumerate() {
        let line = raw.trim_end_matches('\r');
        let start = starts[k];
        let whole = |kind| Unsupported { kind, start, end: start + line.chars().count() };
        match role {
            LineRole::Comment if !found => {
                // `# @name value`, `// @name=value`, other `@directives`.
                let t = line.trim_start().trim_start_matches(['#', '/']).trim_start();
                if let Some(tag) = t.strip_prefix('@') {
                    let word: String = tag.chars().take_while(|c| c.is_alphanumeric() || *c == '-' || *c == '_').collect();
                    let value = tag[word.len()..].trim_start().trim_start_matches('=').trim();
                    if word == "name" {
                        if !value.is_empty() {
                            name = Some(value.to_string());
                        }
                    } else if !word.is_empty() {
                        req.unsupported.push(whole(UnsupportedKind::Directive(word)));
                    }
                }
            }
            LineRole::FileVariable => {
                let var = file_variable(line).unwrap_or_default().to_string();
                req.unsupported.push(whole(UnsupportedKind::FileVariable(var)));
            }
            LineRole::Request => {
                found = true;
                req.line = first_line + k;
                let mut words = line.split_whitespace();
                let first = words.next().unwrap_or_default();
                let upper = first.to_ascii_uppercase();
                let (method, url) = if METHODS.contains(&first) || OTHER_METHODS.contains(&upper.as_str()) {
                    (first.to_string(), words.next().unwrap_or_default().to_string())
                } else {
                    ("GET".to_string(), first.to_string())
                };
                if OTHER_METHODS.contains(&upper.as_str()) {
                    let len = first.chars().count();
                    let lead = line.chars().take_while(|c| c.is_whitespace()).count();
                    req.unsupported.push(Unsupported { kind: UnsupportedKind::Method(upper.clone()), start: start + lead, end: start + lead + len });
                }
                req.method = method;
                req.url = url;
                let rest: Vec<&str> = words.collect();
                if let Some(v) = rest.iter().find(|w| w.to_ascii_uppercase().starts_with("HTTP/")) {
                    req.version = Some(v.to_string());
                }
                variables(line, start, &mut req.unsupported);
            }
            LineRole::UrlContinuation => {
                req.url.push_str(line.trim());
                variables(line, start, &mut req.unsupported);
            }
            LineRole::Header => {
                if let Some((k, v)) = line.split_once(':') {
                    req.headers.push((k.trim().to_string(), v.trim().to_string()));
                }
                variables(line, start, &mut req.unsupported);
            }
            LineRole::Body | LineRole::Blank if found => {
                if *role == LineRole::Body || !body.is_empty() {
                    body.push(line);
                }
                variables(line, start, &mut req.unsupported);
            }
            LineRole::BodyFile => req.unsupported.push(whole(UnsupportedKind::BodyFile)),
            // Reported once, on the line that starts the handler.
            LineRole::Handler if line.starts_with('>') => req.unsupported.push(whole(UnsupportedKind::Handler)),
            LineRole::Redirect => req.unsupported.push(whole(UnsupportedKind::Redirect)),
            _ => {}
        }
    }
    // The blank line that ends the body (before the next `###`) is not part of it.
    while body.last().is_some_and(|l| l.trim().is_empty()) {
        body.pop();
    }
    req.body = body.join("\n");
    let handler_lines: Vec<_> = lines.iter().zip(roles).filter(|(_, role)| **role == LineRole::Handler).map(|(line, _)| line.trim_end_matches('\r')).collect();
    let handler_bytes = handler_lines.iter().fold(0usize, |total, line| total.saturating_add(line.len().saturating_add(1)));
    if handler_bytes <= 64 * 1024 {
        let handler = handler_lines.join("\n");
        req.response_script = handler
            .trim()
            .strip_prefix("> {%")
            .and_then(|body| body.strip_suffix("%}"))
            .filter(|body| !body.contains("%}") && !body.contains("{%"))
            .map(str::to_string);
    }
    req.name = name;
    (req, found)
}

/// The `{{…}}` of `line`, whose first char is at `start`.
fn variables(line: &str, start: usize, out: &mut Vec<Unsupported>) {
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i + 1 < chars.len() {
        if chars[i] == '{' && chars[i + 1] == '{' {
            if let Some(len) = (i + 2..chars.len().saturating_sub(1)).find(|&j| chars[j] == '}' && chars[j + 1] == '}') {
                let name: String = chars[i + 2..len].iter().collect::<String>().trim().to_string();
                out.push(Unsupported { kind: UnsupportedKind::Variable(name), start: start + i, end: start + len + 2 });
                i = len + 2;
                continue;
            }
        }
        i += 1;
    }
}

/// Colours of lines `first..first + count` of an HTTP file, given all its lines.
pub(crate) fn highlight(lines: &[String], first: usize, count: usize) -> Vec<Vec<Span>> {
    let last = (first + count).min(lines.len());
    if first >= last {
        return Vec::new();
    }
    // The request block `first` is in starts at the closest separator above it.
    let floor = first.saturating_sub(MAX_LOOK_BACK);
    let begin = (floor..=first).rev().find(|&l| is_separator(&lines[l])).unwrap_or(floor);
    let roles = line_roles(lines[begin..last].iter().map(String::as_str));
    (first..last).map(|l| line_spans(lines[l].trim_end_matches(['\n', '\r']), roles[l - begin])).collect()
}

fn line_spans(line: &str, role: LineRole) -> Vec<Span> {
    let chars: Vec<char> = line.chars().collect();
    let n = chars.len();
    let mut roles = vec![Hl::Plain; n];
    let lead = chars.iter().take_while(|c| c.is_whitespace()).count();
    match role {
        LineRole::Separator | LineRole::Comment => {
            roles[lead..].fill(Hl::Comment);
            // `@name` and other tags.
            if let Some(at) = chars.iter().position(|&c| c == '@') {
                let end = (at + 1..n).find(|&j| !(chars[j].is_alphanumeric() || chars[j] == '-' || chars[j] == '_')).unwrap_or(n);
                if end > at + 1 {
                    roles[at..end].fill(Hl::Attribute);
                }
            }
        }
        LineRole::FileVariable => {
            let eq = chars.iter().position(|&c| c == '=').unwrap_or(n);
            roles[..eq].fill(Hl::Constant);
            if eq < n {
                roles[eq] = Hl::Operator;
                roles[eq + 1..].fill(Hl::String);
            }
        }
        LineRole::Request => {
            let words = word_ranges(&chars);
            let mut url_word = 0;
            if let Some(&(s, e)) = words.first() {
                let w: String = chars[s..e].iter().collect();
                if METHODS.contains(&w.as_str()) || OTHER_METHODS.contains(&w.to_ascii_uppercase().as_str()) {
                    roles[s..e].fill(Hl::Keyword);
                    url_word = 1;
                }
            }
            if let Some(&(s, e)) = words.get(url_word) {
                roles[s..e].fill(Hl::Link);
            }
            for &(s, e) in words.iter().skip(url_word + 1) {
                let w: String = chars[s..e].iter().collect();
                if w.to_ascii_uppercase().starts_with("HTTP/") {
                    roles[s..e].fill(Hl::Constant);
                }
            }
        }
        LineRole::UrlContinuation => roles[lead..].fill(Hl::Link),
        LineRole::Header => {
            if let Some(colon) = chars.iter().position(|&c| c == ':') {
                roles[lead..colon].fill(Hl::Property);
                roles[colon] = Hl::Punct;
            }
        }
        LineRole::BodyFile | LineRole::Handler | LineRole::Redirect => {
            roles[lead..].fill(Hl::Comment);
            let arrows = chars[lead..].iter().take_while(|&&c| c == '<' || c == '>').count();
            roles[lead..lead + arrows].fill(Hl::Operator);
        }
        LineRole::Body | LineRole::Blank => {}
    }
    // `{{variables}}` anywhere.
    let mut found = Vec::new();
    variables(line, 0, &mut found);
    for u in found {
        roles[u.start..u.end.min(n)].fill(Hl::Preproc);
    }
    let mut spans: Vec<Span> = Vec::new();
    for (col, hl) in roles.into_iter().enumerate() {
        match spans.last_mut() {
            Some(sp) if sp.hl == hl && sp.end == col => sp.end = col + 1,
            _ => spans.push(Span { start: col, end: col + 1, hl }),
        }
    }
    spans.retain(|s| s.hl != Hl::Plain);
    spans
}

/// Char ranges of the whitespace-separated words of `chars`.
fn word_ranges(chars: &[char]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }
        let s = i;
        while i < chars.len() && !chars[i].is_whitespace() {
            i += 1;
        }
        out.push((s, i));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "### List users\nGET https://example.test/users?page=1 HTTP/1.1\nAccept: application/json\n\n###\n# @name create\nPOST http://localhost:8080/users\n    ?notify=true\n    &source=ide\nContent-Type: application/json\n\n{\n  \"name\": \"Ferris\"\n}\n\n\n### Health\nexample.test/health\n";

    #[test]
    fn requests_are_split_and_read() {
        let reqs = parse_requests(FILE);
        assert_eq!(reqs.len(), 3, "{reqs:#?}");
        let r = &reqs[0];
        assert_eq!(
            (r.name.as_deref(), r.line, r.method.as_str(), r.url.as_str(), r.version.as_deref()),
            (Some("List users"), 1, "GET", "https://example.test/users?page=1", Some("HTTP/1.1"))
        );
        assert_eq!(r.headers, vec![("Accept".to_string(), "application/json".to_string())]);
        assert_eq!(r.body, "");
        let r = &reqs[1];
        assert_eq!((r.name.as_deref(), r.line, r.method.as_str()), (Some("create"), 6, "POST"));
        assert_eq!(r.url, "http://localhost:8080/users?notify=true&source=ide");
        assert_eq!(r.headers, vec![("Content-Type".to_string(), "application/json".to_string())]);
        assert_eq!(r.body, "{\n  \"name\": \"Ferris\"\n}");
        assert!(r.unsupported.is_empty());
        let r = &reqs[2];
        assert_eq!((r.name.as_deref(), r.line, r.method.as_str(), r.url.as_str()), (Some("Health"), 17, "GET", "example.test/health"));
        assert_eq!(r.title(), "Health");
    }

    #[test]
    fn blocks_without_a_request_line_are_skipped() {
        assert!(parse_requests("").is_empty());
        assert!(parse_requests("### nothing\n# just a comment\n\n###\n").is_empty());
        let reqs = parse_requests("// comment\r\nDELETE /items/3\r\nHost: localhost\r\n");
        assert_eq!(reqs.len(), 1);
        assert_eq!((reqs[0].method.as_str(), reqs[0].url.as_str(), reqs[0].line), ("DELETE", "/items/3", 1));
        assert_eq!(reqs[0].headers, vec![("Host".to_string(), "localhost".to_string())]);
        assert_eq!(reqs[0].title(), "DELETE /items/3");
    }

    #[test]
    fn variables_scripts_and_file_bodies_are_reported() {
        let text = "@host = localhost\n###\n# @no-redirect\nPOST http://{{host}}/x\nAuthorization: Bearer {{ token }}\n\n< ./body.json\n\n> {%\n  client.global.set(\"a\", 1);\n%}\n>> out.json\n";
        let reqs = parse_requests(text);
        assert_eq!(reqs.len(), 1, "{reqs:#?}");
        let kinds: Vec<_> = reqs[0].unsupported.iter().map(|u| u.kind.clone()).collect();
        assert_eq!(
            kinds,
            vec![
                UnsupportedKind::Directive("no-redirect".into()),
                UnsupportedKind::Variable("host".into()),
                UnsupportedKind::Variable("token".into()),
                UnsupportedKind::BodyFile,
                UnsupportedKind::Handler,
                UnsupportedKind::Redirect,
            ]
        );
        let host = &reqs[0].unsupported[1];
        let chars: Vec<char> = text.chars().collect();
        assert_eq!(chars[host.start..host.end].iter().collect::<String>(), "{{host}}");
        assert_eq!(reqs[0].blockers().count(), 2);
        // A file variable above the first separator is a block of its own, with no request.
        let all: Vec<_> = unsupported_in(text).into_iter().map(|u| u.kind).collect();
        assert_eq!(all[0], UnsupportedKind::Directive("no-redirect".into()));
        assert_eq!(all.len(), 5);
        let reqs = parse_requests("@host = localhost\nGET http://{{host}}/\n");
        assert_eq!(reqs[0].unsupported[0].kind, UnsupportedKind::FileVariable("host".into()));
        let reqs = parse_requests("WEBSOCKET ws://localhost/chat\n");
        assert_eq!(reqs[0].unsupported[0].kind, UnsupportedKind::Method("WEBSOCKET".into()));
        assert!(reqs[0].blockers().next().is_some());
    }

    #[test]
    fn untrusted_text_never_panics() {
        for text in ["{{", "}}{{", "GET {{", "###\n\u{0}\n\u{feff}GET x\n:\n\n> {%", "@=\n@ =x\n", "é{{é}}é\n  ?é\n", "\r\r\n###\r\n"] {
            let _ = parse_requests(text);
            let lines: Vec<String> = text.split_inclusive('\n').map(str::to_string).collect();
            let _ = highlight(&lines, 0, lines.len() + 3);
        }
    }

    #[test]
    fn lines_get_their_colours() {
        let lines: Vec<String> = FILE.split_inclusive('\n').map(str::to_string).collect();
        let spans = highlight(&lines, 0, lines.len());
        let text_of = |l: usize, hl: Hl| -> Vec<String> {
            let chars: Vec<char> = lines[l].chars().collect();
            spans[l].iter().filter(|s| s.hl == hl).map(|s| chars[s.start..s.end].iter().collect()).collect()
        };
        assert_eq!(text_of(0, Hl::Comment), vec!["### List users"]);
        assert_eq!(text_of(1, Hl::Keyword), vec!["GET"]);
        assert_eq!(text_of(1, Hl::Link), vec!["https://example.test/users?page=1"]);
        assert_eq!(text_of(1, Hl::Constant), vec!["HTTP/1.1"]);
        assert_eq!(text_of(2, Hl::Property), vec!["Accept"]);
        assert_eq!(text_of(5, Hl::Attribute), vec!["@name"]);
        assert_eq!(text_of(7, Hl::Link), vec!["?notify=true"]);
        assert!(spans[11].is_empty(), "the body stays plain");
        assert_eq!(text_of(17, Hl::Link), vec!["example.test/health"]);
        // Highlighting from the middle of a block finds its start, also through the editor's
        // entry point.
        let part = highlight(&lines, 9, 2);
        assert_eq!(part[0], spans[9]);
        let rope = rc_text::Rope::from_str(FILE);
        assert_eq!(crate::Lang::from_path(std::path::Path::new("api.rest")), crate::Lang::Http);
        for first in 0..lines.len() {
            let got = crate::highlight_lexical(crate::Lang::Http, &rope, first, 3).unwrap();
            assert_eq!(got[0], spans[first], "line {first}");
        }
        let var: Vec<String> = vec!["GET http://{{host}}/a\n".into()];
        let s = highlight(&var, 0, 1);
        assert!(s[0].iter().any(|s| s.hl == Hl::Preproc && s.end - s.start == 8));
    }
}
