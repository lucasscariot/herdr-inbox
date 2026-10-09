//! Pasted images in the composer and the reply box. Pure.
//!
//! The editor shows each image as `[Image #N]`, the way Claude Code and Codex
//! do. On send the tokens become the saved files' paths, and the agent gets
//! each path pasted on its own: Claude Code and Codex turn a lone pasted image
//! path into an attachment, other agents read the path. A task that names
//! saved images is plain text, so the journal, history and re-send keep them.

/// Saved images live in a directory with this in its path.
pub const DIR_MARKER: &str = "/herdr-inbox/images/";
const EXTENSIONS: &[&str] = &[".png", ".jpg", ".jpeg", ".gif", ".webp"];

/// The editor's placeholder for the `number`th image, counting from 1.
pub fn token(number: usize) -> String {
    format!("[Image #{number}]")
}

/// A path the inbox saved a pasted image to.
pub fn is_saved_image(word: &str) -> bool {
    word.starts_with('/')
        && word.contains(DIR_MARKER)
        && EXTENSIONS.iter().any(|ext| word.to_ascii_lowercase().ends_with(ext))
}

/// Replaces each `[Image #N]` with the path of image N, set apart by spaces
/// so it stays a word of its own. Unknown numbers stay as typed.
pub fn expand(text: &str, images: &[String]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("[Image #") {
        let after = &rest[start + "[Image #".len()..];
        let digits = after.find(|c: char| !c.is_ascii_digit()).unwrap_or(after.len());
        let path = (digits > 0 && after[digits..].starts_with(']'))
            .then(|| after[..digits].parse::<usize>().ok())
            .flatten()
            .and_then(|n| n.checked_sub(1))
            .and_then(|i| images.get(i));
        let Some(path) = path else {
            out.push_str(&rest[..start + 1]);
            rest = &rest[start + 1..];
            continue;
        };
        out.push_str(&rest[..start]);
        if out.chars().next_back().is_some_and(|c| !c.is_whitespace()) {
            out.push(' ');
        }
        out.push_str(path);
        rest = &after[digits + 1..];
        if rest.chars().next().is_some_and(|c| !c.is_whitespace()) {
            out.push(' ');
        }
    }
    out.push_str(rest);
    out
}

/// The reverse of [`expand`]: saved image paths become `[Image #N]` again,
/// numbered in order, with the list of paths.
pub fn collapse(text: &str) -> (String, Vec<String>) {
    let mut images = Vec::new();
    let mut out = String::with_capacity(text.len());
    for segment in segments(text) {
        match segment {
            Segment::Text(text) => out.push_str(text),
            Segment::Image(path) => {
                images.push(path.to_string());
                out.push_str(&token(images.len()));
            }
        }
    }
    (out, images)
}

/// The text without its images, line by line, for titles and branch names.
pub fn strip(text: &str) -> String {
    let text = without_tokens(text);
    let lines = text.lines().map(|line| {
        let words = line.split_whitespace().filter(|w| !is_saved_image(w));
        words.collect::<Vec<_>>().join(" ")
    });
    lines.filter(|line| !line.is_empty()).collect::<Vec<_>>().join("\n")
}

/// The text with every `[Image #N]` removed.
fn without_tokens(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("[Image #") {
        let after = &rest[start + "[Image #".len()..];
        let digits = after.find(|c: char| !c.is_ascii_digit()).unwrap_or(after.len());
        out.push_str(&rest[..start]);
        if digits > 0 && after[digits..].starts_with(']') {
            rest = &after[digits + 1..];
        } else {
            out.push('[');
            rest = &rest[start + 1..];
        }
    }
    out.push_str(rest);
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Segment<'a> {
    Text(&'a str),
    Image(&'a str),
}

/// Splits text at saved image paths, keeping everything else as it is.
pub fn segments(text: &str) -> Vec<Segment<'_>> {
    let mut out = Vec::new();
    let mut text_start = 0;
    let mut word_start = None;
    for (index, c) in text.char_indices().chain(std::iter::once((text.len(), ' '))) {
        match (c.is_whitespace(), word_start) {
            (false, None) => word_start = Some(index),
            (true, Some(start)) => {
                word_start = None;
                if is_saved_image(&text[start..index]) {
                    if start > text_start {
                        out.push(Segment::Text(&text[text_start..start]));
                    }
                    out.push(Segment::Image(&text[start..index]));
                    text_start = index;
                }
            }
            _ => {}
        }
    }
    if text_start < text.len() {
        out.push(Segment::Text(&text[text_start..]));
    }
    out
}

/// Adds a saved image at the editor's cursor as the next `[Image #N]`,
/// numbering from 1 again once no placeholder is left in the text.
pub fn attach(editor: &mut crate::editor::Editor, images: &mut Vec<String>, path: String) {
    if !has_tokens(editor.text()) {
        images.clear();
    }
    images.push(path);
    let before = editor.text()[..editor.cursor()].chars().next_back();
    if before.is_some_and(|c| !c.is_whitespace()) {
        editor.insert(" ");
    }
    editor.insert(&token(images.len()));
    editor.insert(" ");
}

/// Whether the text still holds an `[Image #N]` placeholder. Without one, a
/// new paste numbers images from 1 again.
pub fn has_tokens(text: &str) -> bool {
    text.contains("[Image #")
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "/home/u/.cache/herdr-inbox/images/image-1.png";
    const B: &str = "/home/u/.cache/herdr-inbox/images/image-2.png";

    #[test]
    fn saved_images_are_recognised_by_their_directory_and_extension() {
        assert!(is_saved_image(A));
        assert!(is_saved_image("/x/herdr-inbox/images/shot.JPG"));
        assert!(!is_saved_image("/home/u/Pictures/shot.png"));
        assert!(!is_saved_image("/x/herdr-inbox/images/notes.txt"));
        assert!(!is_saved_image("x/herdr-inbox/images/shot.png"));
    }

    #[test]
    fn tokens_expand_to_paths_set_apart_by_spaces() {
        let images = [A.to_string(), B.to_string()];
        assert_eq!(expand("fix [Image #1] please", &images), format!("fix {A} please"));
        assert_eq!(expand("look:[Image #2]now", &images), format!("look: {B} now"));
        assert_eq!(expand("[Image #1]\n[Image #2]", &images), format!("{A}\n{B}"));
        assert_eq!(expand("[Image #1][Image #1]", &images), format!("{A} {A}"));
    }

    #[test]
    fn unknown_or_malformed_tokens_stay_as_typed() {
        let images = [A.to_string()];
        for text in
            ["[Image #2]", "[Image #0]", "[Image #]", "[Image #1", "[Image #x]", "[Image #9999999999999999999999]"]
        {
            assert_eq!(expand(text, &images), text);
        }
        assert_eq!(expand("[Image #[Image #1]", &images), format!("[Image # {A}"));
    }

    #[test]
    fn collapse_undoes_expand() {
        let images = vec![A.to_string(), B.to_string()];
        let text = "compare [Image #1] with [Image #2]\nthanks";
        assert_eq!(collapse(&expand(text, &images)), (text.to_string(), images));
        assert_eq!(collapse("no images here"), ("no images here".to_string(), vec![]));
    }

    #[test]
    fn segments_split_at_saved_images_only() {
        let text = format!("see {A} and /tmp/other.png\n{B}");
        assert_eq!(
            segments(&text),
            vec![Segment::Text("see "), Segment::Image(A), Segment::Text(" and /tmp/other.png\n"), Segment::Image(B),]
        );
        assert_eq!(segments(A), vec![Segment::Image(A)]);
        assert_eq!(segments("plain"), vec![Segment::Text("plain")]);
        assert_eq!(segments(""), vec![]);
        assert_eq!(segments("é ü"), vec![Segment::Text("é ü")]);
    }

    #[test]
    fn strip_leaves_the_words() {
        assert_eq!(strip(&format!("[Image #1] why is\n{A}\nthis  broken")), "why is\nthis broken");
        assert_eq!(strip("[Image #1]"), "");
        assert_eq!(strip("[Image #] stays"), "[Image #] stays");
    }

    #[test]
    fn attach_numbers_images_and_restarts_once_none_are_left() {
        let mut editor = crate::editor::Editor::default();
        let mut images = vec!["/stale.png".to_string()];
        editor.insert("look");
        attach(&mut editor, &mut images, A.into());
        assert_eq!(editor.text(), "look [Image #1] ");
        attach(&mut editor, &mut images, B.into());
        assert_eq!(editor.text(), "look [Image #1] [Image #2] ");
        assert_eq!(images, [A, B]);
        assert_eq!(expand(editor.text().trim(), &images), format!("look {A} {B}"));

        editor.set("fresh");
        attach(&mut editor, &mut images, B.into());
        assert_eq!(editor.text(), "fresh [Image #1] ");
        assert_eq!(images, [B]);
    }

    #[test]
    fn has_tokens_sees_any_placeholder() {
        assert!(has_tokens("a [Image #3] b"));
        assert!(!has_tokens("an image of a cat"));
    }
}
