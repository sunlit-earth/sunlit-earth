"""The preview harness's page: every variant's renders, notes and metrics.

One HTML file under the harness's directory with its styles and script inline
and nothing fetched from the network. It names the renders and the references
by their paths relative to itself, so it is opened from that directory.
"""

import html
import json
import re
from pathlib import Path

from texture_pipeline.sea_ice_preview import VARIANTS

TEMPLATE = Path(__file__).with_name("sea_ice_preview.html")
"""The page with ``/*DATA*/null`` where the variants go."""

REFERENCES = (
    ("arctic_2019-08-05.jpg", "Top of the World, MODIS, 2019-08-05: Arctic summer"),
    (
        "arctic_2020-05-06.jpg",
        "Three views of the Top of the World, MODIS, 2020-05-06: Arctic late spring",
    ),
    ("antarctic_2018-01-05.jpg", "Antarctic, MODIS, 2018-01-05: Antarctic summer"),
)


def _inline(text: str) -> str:
    text = html.escape(text, quote=False)
    text = re.sub(r"`([^`]+)`", r"<code>\1</code>", text)
    return re.sub(r"\*\*([^*]+)\*\*", r"<strong>\1</strong>", text)


def markdown(text: str) -> str:
    """Convert the subset of Markdown the notes use to HTML.

    Headings, paragraphs, bullet lists nested by two spaces, inline code and
    bold.

    :param text: The Markdown.
    :returns: The HTML.
    """
    out: list[str] = []
    depth = 0
    paragraph: list[str] = []

    def flush() -> None:
        if paragraph:
            out.append(f"<p>{_inline(' '.join(paragraph))}</p>")
            paragraph.clear()

    def close_lists(to: int) -> None:
        nonlocal depth
        while depth > to:
            out.append("</li></ul>")
            depth -= 1

    for line in text.splitlines():
        item = re.match(r"^( *)- (.*)$", line)
        heading = re.match(r"^(#{1,6}) (.*)$", line)
        if item:
            flush()
            level = len(item[1]) // 2 + 1
            if level > depth:
                while depth < level:
                    out.append("<ul><li>")
                    depth += 1
            else:
                close_lists(level)
                out.append("</li><li>")
            out.append(_inline(item[2]))
        elif heading:
            flush()
            close_lists(0)
            level = min(len(heading[1]) + 2, 6)
            out.append(f"<h{level}>{_inline(heading[2])}</h{level}>")
        elif not line.strip():
            flush()
            close_lists(0)
        elif depth:
            out.append(" " + _inline(line.strip()))
        else:
            paragraph.append(line.strip())
    flush()
    close_lists(0)
    return "\n".join(out)


def _variant(root: Path, directory: Path) -> dict | None:
    metrics_path = directory / "metrics.json"
    if not metrics_path.exists():
        return None
    metrics = json.loads(metrics_path.read_text(encoding="utf-8"))
    notes = directory / "notes.md"
    extras = sorted(
        p.relative_to(root).as_posix()
        for p in directory.glob("*.png")
        if p.name.startswith(("mask-change", "imagery-water", "crop-"))
        or "-changes" in p.name
        or p.name.endswith("-coast-blend.png")
    )
    renders = {
        key: {
            "full": f"{directory.name}/{view['render']}",
            "small": f"{directory.name}/{key}-small.jpg",
        }
        for key, view in metrics["views"].items()
    }
    return {
        "name": directory.name,
        "notes": markdown(notes.read_text(encoding="utf-8")) if notes.exists() else "",
        "metrics": metrics,
        "renders": renders,
        "extras": extras,
        "mtime": metrics_path.stat().st_mtime,
    }


def build(root: Path) -> Path:
    """Write ``index.html`` for every variant under the root.

    :param root: The harness's directory.
    :returns: The page's path.
    """
    variants: list[dict] = []
    for directory in sorted(root.iterdir()):
        if directory.is_dir() and (variant := _variant(root, directory)) is not None:
            variants.append(variant)
    order = {name: i for i, name in enumerate(VARIANTS)}
    variants.sort(key=lambda v: (order.get(v["name"], len(order)), v["mtime"]))
    views = []
    if variants:
        views = [
            {"key": key, "label": view["label"]}
            for key, view in variants[0]["metrics"]["views"].items()
        ]
    data = {
        "variants": variants,
        "views": views,
        "references": [
            {"src": f"refs/{name}", "label": label}
            for name, label in REFERENCES
            if (root / "refs" / name).exists()
        ],
    }
    page = root / "index.html"
    payload = json.dumps(data).replace("</", "<\\/")
    template = TEMPLATE.read_text(encoding="utf-8")
    page.write_text(
        template.replace("/*DATA*/null", payload), encoding="utf-8", newline="\n"
    )
    return page
