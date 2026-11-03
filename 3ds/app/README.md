# Native application

## Installation

The release includes two install formats:

- Homebrew Launcher: copy `3ds/anki-3ds/anki-3ds.3dsx` from the ZIP to the same
  location on SD, then open it in Homebrew Launcher.
- HOME Menu: copy `cia/anki-3ds.cia` to SD, select it in FBI and install it.

Both formats use `SD:/3ds/anki-3ds/` for the collection and settings. Back up this
folder before initial hardware testing. Installing the CIA does not import a
collection or credentials.

For an upgrade, replace the `.3dsx` or install the updated CIA. Preserve collection files
and settings. The CA bundle is embedded, so no separate certificate installation
is required. The README, licenses and manifest can remain on your computer.

## Build and startup

`just old3ds-release` builds ARMv6K/VFPv2 `.3dsx` and `.cia` binaries and packages
them with licenses, a manifest and checksums. See the repository README for setup.
First launch creates its SD folder and writes `cacert.pem` if absent. Sending the
`.3dsx` through `3dslink` needs no separate certificate transfer. Existing files
are preserved. Setup errors appear onscreen instead of terminating the app.

The app opens `sdmc:/3ds/anki-3ds/prototype.anki2`. If absent, the original Anki
backend creates an empty collection; you can then sign in and download a collection.
Use disposable data until final hardware and authenticated AnkiWeb verification.

- Decks: D-pad selects; A studies; Left/Right expands or collapses.
- Review: the top screen shows the card front, then the card back after the
  Show Answer binding. The bottom screen shows the deck, optional counters, a
  large Show Answer button, and rating buttons with their intervals and assigned
  buttons. The default review bindings are A Show Answer/Good, Y Again, X Hard,
  R Easy, L Play/Replay audio, ZL Undo, ZR Bury and X Reveal hint/Type answer;
  D-pad Up/Down scroll. Touch ratings and audio buttons also work. Revealed
  cloze answers appear highlighted in their context on the top screen. Tables,
  code blocks, blockquotes, hint pills ("Show Hint N" buttons on the bottom
  screen) and LaTeX/MathJax approximations render inline. Cards with a
  type-answer field show a "Type answer" button; the typed text appears in the
  input box, and the answer screen shows the green/red comparison. Scroll with
  the assigned buttons or the Circle Pad; a subtle scrollbar appears at the
  right edge when the card is taller than the screen.
- Controls: Settings → Controls and keybinds remaps every review action to A, B,
  X, Y, L, R, ZL, ZR, D-pad, Circle Pad or C-stick directions. Bindings that
  would collide on the same review screen are refused, the profile is saved in
  `settings.json`, and Reset to defaults restores the standard layout.
- Font scale: Settings → Font scale offers 85%, 100%, 125%, 150%, 175%, 200%
  and 250%. A advances the setting and Left/Right adjusts it. It scales card
  text, furigana and line spacing, and is saved in `settings.json`. Card text is
  drawn with the bundled Noto Sans JP font (SIL Open Font License), rasterized
  on demand, so no `cjk.bcfnt` needs to be copied to the SD card.
- Appearance: Settings → Theme selects Light, Cream, Dark and Black (AMOLED).
  Accent color selects a UI accent preset. Text spacing offers Compact, Normal
  and Relaxed. Scrolling speed offers Slow, Normal and Fast. Card counts and
  rating intervals can each be hidden, and the toolbar and deck title remain
  configurable. All choices are saved in `settings.json`.
- Add/Edit: select a field for the Nintendo keyboard; X saves, B cancels.
- Browse: upstream search syntax plus quick filters (All, Current deck, Buried,
  Suspended, Leeches). The selected card shows whether it is buried, suspended
  or a leech, and Restore unburies/unsuspends it. X opens the search keyboard.
- Deck options (toolbar or Menu): grouped Daily limits, New cards, Lapses and
  leeches, FSRS and display-order/burying settings for the selected deck.
- Settings → Log in: enter your AnkiWeb email and masked password once. Login
  checks authentication without uploading or downloading a collection. The saved
  login works after restarting. Settings → Log out removes it from this SD card.
- Sync reuses the saved login, or asks for credentials when needed. The text
  collection syncs first and returns to the deck list; media (images and audio)
  continues in the background and can be studied while it runs. A small footer
  shows the current file count, and Settings → Media sync shows the last result;
  A there stops a running sync or starts a media-only sync. Failures remain
  visible in Settings until the next attempt. Deck options, FSRS, leech rules and
  buried/suspended state live in the collection database, so they sync both
  ways. B cancels the collection sync; START cancels and exits after cleanup.
  Full sync requires a direction when both collections contain data, then asks
  twice before replacement.
- Card images referenced by a card render on the top screen, scaled to fit with
  their aspect ratio preserved and included in the vertical scroll. PNG, JPEG,
  BMP and SVG are supported; GIF and animated WebP play inline, and video
  (`[sound:clip.mp4]`, `<video>`, MJPEG AVI) shows a player with Replay and
  Play/Pause controls on the bottom screen. H.264 MP4 and VP8/VP9 WebM cannot
  be decoded on this hardware and show an "Unsupported video" poster instead.
  Settings → Auto-play video starts card video without tapping.
- Card audio appears as tappable Audio buttons on the bottom screen; the Play/
  Replay audio binding cycles the clips and continues automatically. Settings →
  Audio and review selects playback speed (0.75x, 1.0x, 1.25x or 1.5x) and
  whether Replay uses every clip on the card or only the current side. Auto-play
  audio plays the first clip when a card is revealed. MP3, Ogg Vorbis, Opus and
  WAV audio are decoded on the console.
- SELECT opens a two-column Menu of the common review actions (Undo,
  Bury/Suspend, Edit/Add note, Browse, Sync, Statistics, deck options). Settings
  holds the preferences, account and help, so the whole menu fits one page.

English/Spanish follow the console language. Language overrides are stored in
`language.txt`; remove it to follow the system again. The card font scale is
stored in `settings.json`. No timezone or numeric clock
setup is required. The app opens offline immediately. Until the first verified
HTTPS clock response, timestamps follow the console clock; afterwards the saved
correction also applies offline. Clock checks run in the background when Wi-Fi
connects with a stale correction and retry after failures. Login and sync reuse
corrections verified within 30 minutes. Missing, expired or backwards-clock
timestamps trigger calibration; an RTC jump during the session also expires it. The review screen shows the console's local time and battery level in its top-right corner.

The 3DS RTC carries local civil time without a timezone. The adapter obtains UTC
from the Date header of a certificate- and hostname-validated AnkiWeb HTTPS
response, without an account. It corrects UTC seconds and derives the local
scheduling offset in quarter-hour steps. A severely incorrect console date is
rejected; System Settings remains the source of the user's local date/time.
Existing `utc-offset-minutes.txt` files are read for compatibility, never requested
from new users. Successful checks save `clock-correction.json`.

Passwords stay in memory and are cleared after login. Host keys and their sync
endpoint are saved in `sync-auth.json` and `sync-auth-backup.json`. Both contain
plaintext bearer credentials: anyone who can read the SD card can use them. Never
share these files. Logout deletes both slots and any incomplete temporary file.
A network error keeps the token; an upstream authentication rejection removes it
and the next attempt asks for credentials. Two flushed slots preserve the previous
login during replacement; physical SD power-loss durability is unverified.

Full CSS layout is unsupported. Common inline formatting, lists, ruby/furigana
and cloze highlights are rendered. SD removal and power-loss recovery remain
unverified; full download retains a `.pre-sync-*` backup before replacement.

`emulator` is a development feature: it accepts `sync-test.json` for disposable
credentials, endpoint, and CA. Release builds do not read that file. It must never
be included in a hardware package. See [port status](../docs/FEASIBILITY.md).

Card text uses the bundled Noto Sans JP font, so Japanese renders identically on
hardware and under Azahar without depending on the console's regional font.
