#!/usr/bin/env python3
"""Prints the README a package registry shows for devlanes.

usage: readme.py NOTES_FILE

The text is the repository's README.md with NOTES_FILE, the package's own
section, placed before "## Contributing", so a registry page never falls behind
the repository's. npm and PyPI render it away from the repository, where a
relative link or image leads nowhere, so one of those fails the build. Standard
library only.
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
ANCHOR = "\n## Contributing\n"
# A Markdown link or image, or an HTML src or href, that is not an absolute URL
# or an anchor of the page.
RELATIVE = re.compile(r"""\]\((?!https?://|mailto:|#)|\b(?:src|href)=["'](?!https?://|mailto:|#)""")


def package_readme(notes):
    """Returns the repository README with the text of NOTES before its Contributing section."""
    readme = (ROOT / "README.md").read_text(encoding="utf-8")
    if readme.count(ANCHOR) != 1:
        sys.exit('README.md needs exactly one "## Contributing" heading, where the package notes go')
    section = notes.read_text(encoding="utf-8").strip()
    text = readme.replace(ANCHOR, f"\n{section}\n{ANCHOR}")
    for number, line in enumerate(text.splitlines(), 1):
        if RELATIVE.search(line):
            sys.exit(f"line {number} of the package README has a relative link, which breaks on npm and PyPI: {line.strip()}")
    return text


def main(args):
    """Parses the command line."""
    if len(args) != 1 or args[0].startswith("-"):
        print(__doc__.split("\n\n")[1], file=sys.stderr)
        sys.exit(2)
    # Bytes, so Windows does not turn the line ends into CRLF.
    sys.stdout.buffer.write(package_readme(Path(args[0])).encode())


if __name__ == "__main__":
    main(sys.argv[1:])
