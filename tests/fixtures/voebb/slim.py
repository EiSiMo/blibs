#!/usr/bin/env python3
"""Cut the framing out of the voebb.de fixtures without touching what a selector reads.

Removals are byte-exact: the file keeps the server's own markup, minus whole elements.
Nothing is reformatted and no attribute is rewritten. README.md lists what goes and why.

    python3 slim.py start.html results.html …
"""

import re
import sys


def find_tag_end(text, start):
    """Index of the '>' closing the tag that starts at `start`, quotes respected."""
    i = start + 1
    quote = None
    while i < len(text):
        ch = text[i]
        if quote:
            if ch == quote:
                quote = None
        elif ch in "\"'":
            quote = ch
        elif ch == ">":
            return i
        i += 1
    raise ValueError("unterminated tag")


def find_close(text, start, tag):
    """Index just past the `</tag>` matching an already-opened `tag`."""
    depth = 1
    pattern = re.compile(r"</?%s\b" % tag, re.I)
    i = start
    while depth:
        m = pattern.search(text, i)
        if not m:
            raise ValueError("unterminated element <%s>" % tag)
        end = find_tag_end(text, m.start())
        depth += -1 if text[m.start() + 1] == "/" else 1
        i = end + 1
    return i


def spans_of(text, tag, opening=lambda tag: True, void=False, element=lambda html: True):
    """(start, stop) of every `tag` element both predicates accept, in document order."""
    found = []
    i = 0
    opener = re.compile(r"<%s\b" % tag, re.I)
    while True:
        m = opener.search(text, i)
        if not m:
            break
        end = find_tag_end(text, m.start())
        if not opening(text[m.start():end + 1]):
            i = end + 1
            continue
        stop = end + 1 if void else find_close(text, end + 1, tag)
        i = stop
        if element(text[m.start():stop]):
            found.append((m.start(), stop))
    return found


def cut(text, spans):
    """Remove the given spans, back to front so the earlier indices stay valid."""
    for start, stop in reversed(spans):
        text = text[:start] + text[stop:]
    return text


def remove(text, tag, opening=lambda tag: True, void=False, element=lambda html: True):
    return cut(text, spans_of(text, tag, opening, void, element))


def is_prose(html):
    """An accordion is help text only if it holds no form control. The panels that do
    (the sort buttons in `div#R05`, the facet tree in `div#R11`) are part of the form."""
    return not re.search(r"<(input|select|textarea)\b", html, re.I)


def trim_options(text, keep=5):
    """Leave the first `keep` options of every select, except the two vocabularies of the
    advanced form (`SUCH01_*` index, `LOP01_*` boolean), which are what that file proves."""
    spans = []
    for start, stop in spans_of(text, "select"):
        head = find_tag_end(text, start)
        if re.search(r'id="(SUCH01|LOP01)_', text[start:head + 1]):
            continue
        last = text.rindex("</", head + 1, stop)
        options = list(re.finditer(r"<option\b", text[head + 1:last], re.I))
        if len(options) > keep:
            spans.append((head + 1 + options[keep].start(), last))
    return cut(text, spans)


def keep_representative_rows(text):
    """Thin a result list down to the rows that still prove something: the first two, the
    last (whose `rList_num` is the page's last position), and the first row of each
    distinct media-type/traffic-light pair — that vocabulary is what the file documents."""
    spans = spans_of(text, "li", lambda tag: tag.startswith('<li class="rList_li'))
    keep = {0, 1, len(spans) - 1}
    seen = set()
    for index, (start, stop) in enumerate(spans):
        row = text[start:stop]
        kind = tuple(re.findall(r'class="icon" src="[^"]*" alt="([^"]*)"', row)[:2])
        if kind not in seen:
            seen.add(kind)
            keep.add(index)
    return cut(text, [span for index, span in enumerate(spans) if index not in keep])


def keep_branch_rubric_only(text):
    """Drop the facet rubrics other than `Bibliothek`, keeping the active-filter heading."""
    return remove(
        text,
        "li",
        lambda tag: tag == '<li class="cbtree_branch_li">',
        element=lambda html: "</i>Bibliothek<" not in html[:600],
    )


def slim(text):
    text = remove(text, "script")
    text = remove(text, "link", void=True)
    text = remove(text, "meta", lambda tag: "charset" not in tag, void=True)
    text = remove(text, "header", lambda tag: 'id="header"' in tag)
    text = remove(text, "footer", lambda tag: 'id="footer"' in tag)
    text = remove(text, "nav", lambda tag: 'id="sprunglinks"' in tag)
    text = remove(text, "div", lambda tag: 'class="accordion"' in tag, element=is_prose)
    text = remove(text, "div", lambda tag: 'id="adis-timeout-counter"' in tag)
    text = remove(text, "ul", lambda tag: "splide__list" in tag)
    text = remove(text, "div", lambda tag: 'class="rList_cover"' in tag)
    text = remove(text, "div", lambda tag: "rList_col rList_check" in tag)
    text = remove(text, "div", lambda tag: "rList_col rList_button" in tag)
    text = remove(text, "div", lambda tag: "rList_col rList_www" in tag)
    text = remove(text, "div", lambda tag: "rList_panel" in tag)
    return trim_options(text)


if __name__ == "__main__":
    for path in sys.argv[1:]:
        with open(path, encoding="utf-8") as handle:
            before = handle.read()
        after = slim(before)
        if path.endswith(("results_filtered.html", "results_filtered_page2.html")):
            after = keep_representative_rows(after)
        if path.endswith(("results_filtered.html", "results_filtered_page2.html", "results_isbn.html")):
            after = keep_branch_rubric_only(after)
        with open(path, "w", encoding="utf-8") as handle:
            handle.write(after)
        print("%-32s %7d -> %7d" % (path, len(before.encode()), len(after.encode())))
