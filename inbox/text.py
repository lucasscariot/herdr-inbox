"""Terminal text measurement, wrapping, matching, and formatting helpers."""

import unicodedata
from pathlib import Path


def clean(text):
    return "".join(c for c in str(text) if c.isprintable())


def cell_width(character):
    return 0 if unicodedata.combining(character) else 2 if unicodedata.east_asian_width(character) in ("W", "F") else 1


def width_of(text):
    return sum(cell_width(c) for c in text)


def clip(text, width):
    result, used = "", 0
    for character in clean(text):
        size = cell_width(character)
        if used + size > width:
            break
        result += character
        used += size
    return result


def ellipsis(text, width):
    """Clip to ``width`` cells, marking a cut with a single ellipsis."""
    text = clean(text)
    if width_of(text) <= width:
        return text
    return clip(text, max(0, width - 1)) + "…" if width > 0 else ""


def pad(text, width):
    text = clip(text, width)
    return text + " " * (width - width_of(text))


def task_layout(task, cursor, width):
    """Wrap terminal cells at word boundaries and retain the insertion point.

    Returns the visual lines, the cursor's (row, column), and every character's
    position. Soft wraps keep their trailing space on the upper line; a word
    longer than the width breaks wherever it must.
    """
    lines, positions = [], []
    start, length = 0, len(task)
    while True:
        column, index, last_space = 0, start, -1
        while index < length and task[index] != "\n":
            size = cell_width(task[index])
            if column + size > width:
                break
            if task[index] == " ":
                last_space = index
            column += size
            index += 1
        if index < length and task[index] != "\n":
            if last_space >= 0 and last_space + 1 < index:
                index = last_space + 1
            end = next_start = index
            hard_newline = False
        elif index < length:
            end, next_start, hard_newline = index, index + 1, True
        else:
            end, next_start, hard_newline = length, None, False
        column = 0
        for position in range(start, end):
            positions.append((len(lines), column))
            column += cell_width(task[position])
        if hard_newline:
            positions.append((len(lines), column))
        lines.append(task[start:end])
        if next_start is None:
            if column >= width and width > 0:
                lines.append("")
                column = 0
            positions.append((len(lines) - 1, column))
            break
        start = next_start
    return lines, positions[cursor], positions


def match_score(query, text):
    """Rank ``text`` for ``query``: lower is better, None means no match.

    Exact prefixes win, then word starts, then substrings, then scattered
    subsequences so a few typed letters reach a long model ID.
    """
    query, text = query.strip().lower(), text.lower()
    if not query:
        return 0
    position = text.find(query)
    if position == 0:
        return 0
    if position > 0:
        return 1 if text[position - 1] in " -_/.:·" else 2 + position / 1000
    cursor, gaps, previous = 0, 0, -1
    for character in query:
        found = text.find(character, cursor)
        if found < 0:
            return None
        if previous >= 0 and found != previous + 1:
            gaps += 1
        previous, cursor = found, found + 1
    return 10 + gaps + previous / 1000


def rank(query, choices, key=lambda choice: choice[0]):
    scored = []
    for index, choice in enumerate(choices):
        score = match_score(query, key(choice))
        if score is not None:
            scored.append((score, index, choice))
    return [choice for _, _, choice in sorted(scored, key=lambda item: item[:2])]


def tilde(path):
    home = str(Path.home())
    return "~" + path[len(home):] if path == home or path.startswith(home + "/") else path


def age(seconds):
    if seconds is None or seconds < 0:
        return ""
    if seconds < 60:
        return str(int(seconds)) + "s"
    if seconds < 3600:
        return str(int(seconds // 60)) + "m"
    if seconds < 86400:
        return str(int(seconds // 3600)) + "h"
    return str(int(seconds // 86400)) + "d"
