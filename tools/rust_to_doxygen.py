#!/usr/bin/env python3
"""Generate Doxygen input pages from documented production Rust declarations."""

from __future__ import annotations

import argparse
import pathlib
import re
import shutil
import sys


DECLARATION = re.compile(
    r"^\s*(?:pub(?:\([^)]*\))?\s+)?"
    r"(?:(?:unsafe|const|async)\s+|extern\s+\"[^\"]+\"\s+)*"
    r"(?P<kind>fn|struct|enum|const|static|type|trait)\s+"
    r"(?P<name>[A-Za-z_][A-Za-z0-9_]*)"
)


def documented_declarations(source: pathlib.Path) -> tuple[list[str], list[tuple[str, str, list[str]]]]:
    """Return module prose and every documented declaration in one Rust file."""
    module_docs: list[str] = []
    declarations: list[tuple[str, str, list[str]]] = []
    pending: list[str] = []
    for line_number, line in enumerate(source.read_text(encoding="utf-8").splitlines(), start=1):
        stripped = line.strip()
        if stripped.startswith("//!"):
            module_docs.append(stripped[3:].lstrip())
            continue
        if stripped.startswith("///"):
            pending.append(stripped[3:].lstrip())
            continue
        match = DECLARATION.match(line)
        if match:
            if not pending:
                raise ValueError(f"{source}:{line_number}: undocumented {match.group('kind')} {match.group('name')}")
            declarations.append((match.group("kind"), match.group("name"), pending))
            pending = []
            continue
        if stripped and not stripped.startswith("#["):
            pending = []
    return module_docs, declarations


def write_page(output: pathlib.Path, source: pathlib.Path) -> None:
    """Write one Doxygen page with checked Rust API prose and source context."""
    module_docs, declarations = documented_declarations(source)
    relative = source.as_posix()
    page_id = "rust_" + "_".join(source.with_suffix("").parts)
    lines = [
        "/**",
        f" * @page {page_id} Rust source: {relative}",
        " * @brief Rust implementation reference.",
        " *",
    ]
    if module_docs:
        lines.extend(f" * {line}" if line else " *" for line in module_docs)
        lines.append(" *")
    lines.append(" * @section " + page_id + "_declarations Documented declarations")
    for kind, name, docs in declarations:
        lines.append(f" * @par `{kind} {name}`")
        lines.extend(f" * {line}" if line else " *" for line in docs)
    lines.extend(
        [
            " *",
            " * @section " + page_id + "_source Source context",
            " * The complete implementation, including fields and implementation blocks,",
            " * follows. Rustdoc publishes the symbol-level reference for the same source.",
            " * @code{.rs}",
        ]
    )
    lines.extend(f" * {line}" for line in source.read_text(encoding="utf-8").splitlines())
    lines.extend([" * @endcode", " */"])
    output.write_text("\n".join(lines) + "\n", encoding="utf-8")


def main() -> int:
    """Generate input pages and fail when production Rust loses documentation."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=pathlib.Path)
    parser.add_argument("sources", nargs="+", type=pathlib.Path)
    args = parser.parse_args()
    if args.output.exists():
        shutil.rmtree(args.output)
    args.output.mkdir(parents=True)
    for source in args.sources:
        write_page(args.output / f"{source.stem}.dox", source)
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except ValueError as error:
        print(error, file=sys.stderr)
        raise SystemExit(1) from error
