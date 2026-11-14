# Copyright: Anki 3DS contributors
# License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

"""Create Homebrew Launcher metadata and the 3D HOME Menu banner."""

import subprocess
from pathlib import Path

root = Path(__file__).resolve().parents[2]
out = root / "out/3ds/app"
out.mkdir(parents=True, exist_ok=True)
resources = root / "3ds/app/resources"
subprocess.run(
    [
        str(root / "out/3ds/tools/bin/bannertool"),
        "makesmdh",
        "-s",
        "Anki 3DS",
        "-l",
        "Anki flashcards for Nintendo 3DS",
        "-p",
        "Anki 3DS contributors",
        "-i",
        str(resources / "icon.png"),
        "-o",
        str(out / "anki-3ds.smdh"),
    ],
    check=True,
)
subprocess.run(
    [
        str(root / "out/3ds/tools/bin/bannertool"),
        "makebanner",
        "-ci",
        str(resources / "banner.cgfx"),
        "-a",
        str(resources / "banner.wav"),
        "-o",
        str(out / "banner.bnr"),
    ],
    check=True,
)
