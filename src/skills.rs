//! `$skill` mentions in the composer and the reply box. Pure.
//!
//! Typing `$` lists the skills installed on the target machine, the way Codex
//! does; picking one writes `$name`. Harnesses disagree on how a skill is
//! invoked (`$name`, `/name`, `/skill:name`, or only by the model) and on
//! where they look for one, so the text keeps `$name` as written and the
//! message gets a short footer naming each mentioned skill's `SKILL.md`.
//! Every agent can read a file, so a skill works whichever harness runs the
//! task, even one that never heard of the directory it lives in.

use crate::discovery::Skill;
use crate::editor::Editor;

/// The most skills the menu lists for one query.
pub const MENU_LIMIT: usize = 50;

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':')
}

/// The `$` mention being typed: where its `$` is, and the letters after it,
/// when the cursor ends a word that starts with `$`.
pub fn query(editor: &Editor) -> Option<(usize, &str)> {
    let before = &editor.text()[..editor.cursor()];
    let start = before.rfind(|c: char| !is_name_char(c))?;
    if !before[start..].starts_with('$') {
        return None;
    }
    let preceded = before[..start].chars().next_back();
    if preceded.is_some_and(|c| !c.is_whitespace() && !"([{\"'`".contains(c)) {
        // `a$b`, `$$`: not a mention.
        return None;
    }
    Some((start, &before[start + 1..]))
}

/// Skills for the query, best match first.
pub fn matching<'a>(skills: &'a [Skill], query: &str) -> Vec<&'a Skill> {
    let mut found = crate::fuzzy::rank(query, skills, |s| s.name.clone());
    found.truncate(MENU_LIMIT);
    found
}

/// Writes `$name ` in place of the mention being typed.
pub fn accept(editor: &mut Editor, start: usize, name: &str) {
    editor.replace_before_cursor(start, &format!("${name} "));
}

/// The completion menu's state. Its content follows the text: it shows
/// while the cursor ends a `$` word, unless that word was dismissed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Menu {
    pub selected: usize,
    /// The `$` the user closed the menu on with Esc, and the text it was in
    /// (see [`Editor::replaced`]).
    dismissed: Option<(usize, u64)>,
}

impl Menu {
    /// The skills offered right now, with the mention's start.
    pub fn offered<'a>(&self, editor: &Editor, skills: &'a [Skill]) -> Option<(usize, Vec<&'a Skill>)> {
        let (start, query) = query(editor)?;
        if self.dismissed == Some((start, editor.replaced())) {
            return None;
        }
        let found = matching(skills, query);
        (!found.is_empty()).then_some((start, found))
    }

    /// Forgets a dismissal once its `$` is deleted or its text replaced, so a
    /// later mention at the same place opens the menu again.
    pub fn follow(&mut self, editor: &Editor) {
        if let Some((start, replaced)) = self.dismissed
            && (replaced != editor.replaced() || !editor.text().get(start..).is_some_and(|rest| rest.starts_with('$')))
        {
            self.dismissed = None;
        }
    }

    pub fn dismiss(&mut self, editor: &Editor) {
        self.dismissed = query(editor).map(|(start, _)| (start, editor.replaced()));
        self.selected = 0;
    }

    /// Moves the highlight, wrapping around the ends.
    pub fn step(&mut self, count: usize, forward: bool) {
        if count > 0 {
            let current = self.selected.min(count - 1);
            self.selected = if forward { (current + 1) % count } else { (current + count - 1) % count };
        }
    }

    /// Writes the highlighted skill. False when the menu is not showing.
    pub fn accept(&mut self, editor: &mut Editor, skills: &[Skill]) -> bool {
        let Some((start, found)) = self.offered(editor, skills) else {
            return false;
        };
        let name = found[self.selected.min(found.len() - 1)].name.clone();
        accept(editor, start, &name);
        self.selected = 0;
        true
    }

    /// After an edit: a new query starts at the top of the list.
    pub fn edited(&mut self) {
        self.selected = 0;
    }
}

/// The installed skills a text mentions as `$name`, in order, once each.
/// A word that names no skill (`$HOME`, `$5`) stays plain text.
pub fn mentioned<'a>(text: &str, skills: &'a [Skill]) -> Vec<&'a Skill> {
    let mut found: Vec<&Skill> = Vec::new();
    let mut previous: Option<char> = None;
    for (index, c) in text.char_indices() {
        let boundary = previous.is_none_or(|p| p.is_whitespace() || "([{\"'`".contains(p));
        previous = Some(c);
        if c != '$' || !boundary {
            continue;
        }
        let rest = &text[index + 1..];
        let mut word = &rest[..rest.find(|c: char| !is_name_char(c)).unwrap_or(rest.len())];
        // Sentence punctuation after a mention is not part of its name.
        loop {
            if let Some(skill) = skills.iter().find(|s| s.name == word) {
                if !found.iter().any(|f| f.name == skill.name) {
                    found.push(skill);
                }
                break;
            }
            match word.strip_suffix(['.', ':', '-']) {
                Some(shorter) if !shorter.is_empty() => word = shorter,
                _ => break,
            }
        }
    }
    found
}

/// The message an agent gets: the text as written, then where each
/// mentioned skill lives, so any harness can read and follow it.
pub fn expand(text: &str, skills: &[Skill]) -> String {
    let mentioned = mentioned(text, skills);
    if mentioned.is_empty() {
        return text.to_string();
    }
    let mut out = text.trim_end().to_string();
    out.push_str("\n\nSkills mentioned above: read each SKILL.md and follow it.");
    for skill in mentioned {
        out.push_str(&format!("\n- ${}: {}", skill.name, skill.path));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill(name: &str) -> Skill {
        Skill {
            name: name.into(),
            description: format!("about {name}"),
            path: format!("/h/.agents/skills/{name}/SKILL.md"),
        }
    }

    fn skills() -> Vec<Skill> {
        vec![skill("arena"), skill("architect"), skill("deslop"), skill("code-review")]
    }

    fn editor(text: &str) -> Editor {
        let mut editor = Editor::default();
        editor.set(text);
        editor
    }

    #[test]
    fn a_dollar_word_at_the_cursor_is_a_query() {
        assert_eq!(query(&editor("fix it with $ar")), Some((12, "ar")));
        assert_eq!(query(&editor("$")), Some((0, "")));
        assert_eq!(query(&editor("($code-r")), Some((1, "code-r")));
        assert_eq!(query(&editor("cost a$b")), None, "inside a word");
        assert_eq!(query(&editor("$arena done")), None, "the cursor left the word");
        assert_eq!(query(&editor("plain")), None);
        let mut moved = editor("$arena now");
        moved.home();
        assert_eq!(query(&moved), None);
    }

    #[test]
    fn the_menu_completes_the_highlighted_skill() {
        let skills = skills();
        let mut text = editor("please $ar");
        let mut menu = Menu::default();
        let (start, found) = menu.offered(&text, &skills).unwrap();
        assert_eq!(start, 7);
        let names: Vec<&str> = found.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["arena", "architect"]);
        menu.step(found.len(), true);
        assert!(menu.accept(&mut text, &skills));
        assert_eq!(text.text(), "please $architect ");
        assert_eq!(text.cursor(), text.text().len());
        assert!(menu.offered(&text, &skills).is_none(), "a space ends the mention");
    }

    #[test]
    fn escape_dismisses_until_another_mention_starts() {
        let skills = skills();
        let mut text = editor("cost $");
        let mut menu = Menu::default();
        assert!(menu.offered(&text, &skills).is_some());
        menu.dismiss(&text);
        assert!(menu.offered(&text, &skills).is_none());
        text.insert("5");
        assert!(menu.offered(&text, &skills).is_none(), "typing on stays dismissed");
        assert!(!menu.accept(&mut text, &skills));
        text.insert(" and $");
        assert!(menu.offered(&text, &skills).is_some(), "a new $ opens it again");
    }

    #[test]
    fn a_dismissal_ends_with_its_dollar() {
        let skills = skills();
        let mut text = editor("$");
        let mut menu = Menu::default();
        menu.dismiss(&text);
        menu.follow(&text);
        assert!(menu.offered(&text, &skills).is_none(), "still the dismissed $");
        text.clear();
        menu.follow(&text);
        text.insert("$");
        assert!(menu.offered(&text, &skills).is_some(), "a new task's $ at the same place");
        menu.dismiss(&text);
        text.set("$");
        assert!(menu.offered(&text, &skills).is_some(), "another text, as from the history");
    }

    #[test]
    fn no_skill_no_menu() {
        let mut menu = Menu::default();
        assert!(menu.offered(&editor("$ar"), &[]).is_none());
        assert!(menu.offered(&editor("$zzz"), &skills()).is_none());
        menu.step(0, true);
        assert_eq!(menu.selected, 0);
    }

    #[test]
    fn only_installed_skills_are_mentions() {
        let skills = skills();
        let text = "Use $arena, then $deslop. Not $HOME, a$arena or $5; $arena again and ($code-review).";
        let names: Vec<&str> = mentioned(text, &skills).iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["arena", "deslop", "code-review"]);
    }

    #[test]
    fn the_message_lists_each_mentioned_skill_file() {
        let skills = skills();
        assert_eq!(
            expand("$arena the login page\n", &skills),
            "$arena the login page\n\nSkills mentioned above: read each SKILL.md and follow it.\n- $arena: /h/.agents/skills/arena/SKILL.md"
        );
        assert_eq!(expand("costs $5", &skills), "costs $5", "no mention, untouched");
    }
}
