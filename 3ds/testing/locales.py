# Copyright: Anki 3DS contributors
# License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

"""Validate catalog coverage and translated formatting placeholders."""

import json
import re
from pathlib import Path

root = Path(__file__).resolve().parents[1] / "app"
locales = {
    path.stem: json.loads(path.read_text(encoding="utf-8"))
    for path in sorted((root / "locales").glob("*.json"))
}
assert "en" in locales, "missing English catalog"
en = locales["en"]
for lang, catalog in locales.items():
    assert en.keys() == catalog.keys(), f"language catalogs differ: {lang}"
    for key in en:
        assert re.findall(r"\{\d+\}", en[key]) == re.findall(
            r"\{\d+\}", catalog[key]
        ), (lang, key)
        assert en[key] and catalog[key], (lang, key)
for path in (root / "src").glob("*.rs"):
    for literal in re.findall(
        r'(?:tr|i18n::format)\(\s*("(?:[^"\\]|\\.)*")', path.read_text()
    ):
        assert json.loads(literal) in en, (path, literal)
print(
    f"PASS: {len(en)} messages, {len(locales)} catalogs, all formatting placeholders and source references"
)
