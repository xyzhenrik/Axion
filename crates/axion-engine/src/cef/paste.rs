//! Pasting without Chromium's clipboard.
//!
//! Off-screen Chromium on Wayland has no surface with keyboard focus, and Wayland only shows
//! the clipboard to the focused client: Ctrl+V in a page finds it empty. The browser reads the
//! clipboard itself and the page gets it here, as a `paste` event carrying the data (what
//! upload fields, chat boxes and editors listen to), followed by inserting the text if the
//! page leaves the paste to the browser.

use crate::PasteData;

/// The script that pastes `data` into the focused element of the frame it runs in.
pub(super) fn script(data: &PasteData) -> String {
    let text = data.text.as_deref().map_or_else(|| "null".to_owned(), js_string);
    let html = data.html.as_deref().map_or_else(|| "null".to_owned(), js_string);
    let files: Vec<String> = data
        .files
        .iter()
        .map(|f| format!("[{},{},\"{}\"]", js_string(&f.name), js_string(&f.mime), base64(&f.bytes)))
        .collect();
    format!(
        r#"(() => {{
  const text = {text}, html = {html}, files = [{files}];
  const bytes = (b) => Uint8Array.fromBase64 ? Uint8Array.fromBase64(b) : Uint8Array.from(atob(b), (c) => c.charCodeAt(0));
  let target = document.activeElement || document.body;
  while (target && target.shadowRoot && target.shadowRoot.activeElement) target = target.shadowRoot.activeElement;
  if (!target) return;
  const data = new DataTransfer();
  if (text !== null) data.setData('text/plain', text);
  if (html !== null) data.setData('text/html', html);
  const made = files.map(([name, type, b]) => new File([bytes(b)], name, {{ type }}));
  for (const file of made) data.items.add(file);
  const event = new ClipboardEvent('paste', {{ clipboardData: data, bubbles: true, cancelable: true, composed: true }});
  if (!target.dispatchEvent(event)) return;
  const field = target.matches && target.matches('textarea, input:not([type]), input[type=text], input[type=search], input[type=url], input[type=email], input[type=tel], input[type=password], input[type=number]');
  if (!field && !target.isContentEditable) return;
  if (text) {{ document.execCommand('insertText', false, text); return; }}
  const image = made.find((f) => f.type.startsWith('image/'));
  if (image && target.isContentEditable) document.execCommand('insertImage', false, URL.createObjectURL(image));
}})();"#,
        files = files.join(",")
    )
}

/// A JavaScript string literal.
fn js_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // Line terminators in JavaScript, and everything invisible.
            c if c.is_control() || c == '\u{2028}' || c == '\u{2029}' => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], chunk.get(1).copied().unwrap_or(0), chunk.get(2).copied().unwrap_or(0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 0x3F) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PastedFile;

    #[test]
    fn base64_matches_the_standard() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(&[0xFF, 0xFE, 0xFD, 0x00]), "//79AA==");
    }

    #[test]
    fn strings_cannot_break_out_of_the_literal() {
        assert_eq!(js_string("a\"b\\c\nd"), r#""a\"b\\c\nd""#);
        assert_eq!(js_string("\u{2028}</script>"), "\"\\u2028</script>\"");
        assert_eq!(js_string("\u{0}"), r#""\u0000""#);
    }

    #[test]
    fn script_carries_text_and_files() {
        let data = PasteData {
            text: Some("hi".into()),
            html: None,
            files: vec![PastedFile {
                name: "image.png".into(),
                mime: "image/png".into(),
                bytes: vec![1, 2, 3],
            }],
        };
        let script = script(&data);
        assert!(
            script.contains(r#"const text = "hi", html = null, files = [["image.png","image/png","AQID"]];"#)
        );
    }
}
