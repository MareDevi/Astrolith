//! Bounded streaming detection of the local URL Astro prints when ready.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use url::Url;

/// Bytes carried between drains so a token split across chunks is not lost.
const READY_SCAN_CARRY: usize = 512;

/// A bounded observer fed directly from a process output stream.
///
/// The observer sees every drained chunk, independent of the bounded retained
/// log. Only the unfinished token and the latest local URL candidate are kept.
#[derive(Clone, Debug)]
pub(crate) struct ReadinessObserver {
    active: Arc<AtomicBool>,
    state: Arc<Mutex<ReadinessState>>,
}

#[derive(Debug, Default)]
struct ReadinessState {
    carry: String,
    candidate: Option<Url>,
}

impl Default for ReadinessObserver {
    fn default() -> Self {
        Self {
            active: Arc::new(AtomicBool::new(true)),
            state: Arc::default(),
        }
    }
}

impl ReadinessObserver {
    pub(crate) fn push(&self, chunk: &[u8]) {
        if !self.active.load(Ordering::Acquire) {
            return;
        }
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if !self.active.load(Ordering::Acquire) {
            return;
        }
        let mut text = std::mem::take(&mut state.carry);
        text.push_str(&String::from_utf8_lossy(chunk));

        let (complete, carry) = split_complete(&text);
        if let Some(url) = parse_ready_candidate(complete) {
            state.candidate = Some(url);
        }
        state.carry = carry;
    }

    pub(crate) fn urls(&self, fallback_port: u16) -> Vec<Url> {
        let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let mut urls = Vec::with_capacity(2);
        if let Some(url) = parse_ready_candidate(&state.carry)
            .and_then(|url| url_with_fallback(url, fallback_port))
        {
            urls.push(url);
        }
        if let Some(candidate) = state
            .candidate
            .clone()
            .and_then(|url| url_with_fallback(url, fallback_port))
            && !urls.contains(&candidate)
        {
            urls.push(candidate);
        }
        urls
    }

    pub(crate) fn stop(&self) {
        self.active.store(false, Ordering::Release);
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.carry.clear();
        state.candidate = None;
    }
}

/// Split text into complete tokens and the bounded unfinished suffix.
fn split_complete(text: &str) -> (&str, String) {
    let trimmed_end = text.trim_end_matches(is_token_delimiter).len();
    if trimmed_end < text.len() {
        return (&text[..trimmed_end], String::new());
    }
    let start = text[..trimmed_end]
        .rfind(is_token_delimiter)
        .map_or(0, |index| {
            index + text[index..].chars().next().map_or(1, char::len_utf8)
        });
    (&text[..start], bounded_suffix(&text[start..]))
}

fn bounded_suffix(suffix: &str) -> String {
    let mut start = suffix.len().saturating_sub(READY_SCAN_CARRY);
    while start < suffix.len() && !suffix.is_char_boundary(start) {
        start += 1;
    }
    suffix[start..].to_owned()
}

fn is_token_delimiter(character: char) -> bool {
    character.is_whitespace() || matches!(character, '"' | '\'' | '<' | '>' | '(' | ')' | '[' | ']')
}

/// Strip ANSI SGR/CSI escape sequences so readiness parsing sees clean text.
pub(crate) fn strip_ansi(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for next in chars.by_ref() {
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        output.push(character);
    }
    output
}

/// Parse the first loopback preview URL Astro prints once it is listening.
#[cfg(test)]
pub(crate) fn parse_ready_url(text: &str, fallback_port: u16) -> Option<Url> {
    url_with_fallback(parse_ready_candidate(text)?, fallback_port)
}

fn url_with_fallback(mut url: Url, fallback_port: u16) -> Option<Url> {
    if url.port().is_none() {
        url.set_port(Some(fallback_port)).ok()?;
    }
    Some(url)
}

fn parse_ready_candidate(text: &str) -> Option<Url> {
    let cleaned = strip_ansi(text);
    for token in cleaned.split(is_token_delimiter) {
        let Some(index) = token.find("http://") else {
            continue;
        };
        let raw = token[index..].trim_end_matches([',', ';', '.']);
        let Ok(url) = Url::parse(raw) else {
            continue;
        };
        let local = matches!(url.host_str(), Some("127.0.0.1") | Some("localhost"));
        if !local {
            continue;
        }
        let normalized = match url.port() {
            Some(port) => format!("http://127.0.0.1:{port}/"),
            None => "http://127.0.0.1/".to_owned(),
        };
        return Url::parse(&normalized).ok();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_and_ansi_colored_ready_lines() {
        let plain = "  Local    http://localhost:4321/\n";
        assert_eq!(
            parse_ready_url(plain, 4321).unwrap().as_str(),
            "http://127.0.0.1:4321/"
        );

        let colored = "\u{1b}[32m┃ Local\u{1b}[0m    \u{1b}[36mhttp://127.0.0.1:5199/\u{1b}[0m\n";
        assert_eq!(
            parse_ready_url(colored, 4321).unwrap().as_str(),
            "http://127.0.0.1:5199/"
        );
    }

    #[test]
    fn ignores_network_urls_and_non_local_hosts() {
        let text = "Network use --host to expose\nhttp://192.168.1.10:4321/\n";
        assert!(parse_ready_url(text, 4321).is_none());
    }

    #[test]
    fn ignores_trailing_punctuation_after_url() {
        let text = "Local: http://127.0.0.1:4321/, ready";
        assert_eq!(
            parse_ready_url(text, 1).unwrap().as_str(),
            "http://127.0.0.1:4321/"
        );
    }

    #[test]
    fn observes_a_url_split_across_drains() {
        let observer = ReadinessObserver::default();

        observer.push(b"  Local    ht");
        assert!(
            observer.urls(4321).is_empty(),
            "an incomplete URL is not ready"
        );

        observer.push(b"tp://127.0.0.1:4321/\n");
        assert_eq!(
            observer.urls(4321),
            [Url::parse("http://127.0.0.1:4321/").unwrap()]
        );
    }

    #[test]
    fn observes_a_url_flushed_without_a_trailing_newline() {
        let observer = ReadinessObserver::default();
        observer.push(b"  Local    http://127.0.0.1:4321/");

        assert_eq!(
            observer.urls(4321),
            [Url::parse("http://127.0.0.1:4321/").unwrap()]
        );
    }

    #[test]
    fn observes_a_url_after_a_large_chunk_without_retaining_the_prefix() {
        let observer = ReadinessObserver::default();
        let prefix = vec![b'a'; 2 * 1024 * 1024];

        observer.push(&prefix);
        observer.push(b"\n  Local    http://127.0.0.1:4321/\n");

        assert_eq!(
            observer.urls(4321),
            [Url::parse("http://127.0.0.1:4321/").unwrap()]
        );
        assert!(
            observer
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .carry
                .is_empty()
        );
    }

    #[test]
    fn stopping_observation_ignores_later_output() {
        let observer = ReadinessObserver::default();
        observer.stop();
        observer.push(b"  Local    http://127.0.0.1:4321/\n");

        assert!(observer.urls(4321).is_empty());
    }
}
