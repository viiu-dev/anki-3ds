# <img width="100%" alt="preview-banner-3d" src="https://github.com/user-attachments/assets/3626c80e-59b9-4296-bea3-ec64e99b8bb9" />

A port of [Anki](https://apps.ankiweb.net) to the Nintendo 3DS.

> [!NOTE]
> Developed and tested primarily on **New Nintendo 3DS** hardware. It may run on Old 3DS / 2DS models, but performance, memory stability, and full compatibility on Old 3DS are not guaranteed.

## About

Anki is a spaced repetition program. This project brings it to the Nintendo 3DS with AnkiWeb synchronization, card images, and audio playback. See the official [Anki website](https://apps.ankiweb.net) to learn more about Anki itself.

## Preview

<img width="400" alt="anki-3ds-preview-small" src="https://github.com/user-attachments/assets/e2ffbaf6-e796-4cae-9865-aa1321c603ee" />

## Install

Download the latest `.cia` or `.3dsx` from [Releases](https://github.com/viiu-dev/anki-3ds/releases), or install directly via **FBI** by scanning the QR code below:

<img width="200" alt="Install QR code" src="3ds/docs/images/install-qr.png" />

- **CIA:** Install with FBI and launch from the HOME Menu.
- **3DSX:** Copy to `SD:/3ds/anki-3ds/anki-3ds.3dsx` and launch via Homebrew Launcher.

## Usage

- Press **SELECT** at any time to open the menu (**Log in**, **Sync**, **Settings**, **Browse**, etc.).
- Collection data, saved session tokens, and settings are stored in `SD:/3ds/anki-3ds/`.
- **Audio:** Card audio requires `SD:/3ds/dspfirm.cdc` on real hardware (dump it once in Luma3DS via Rosalina menu: `L + Down + Select` → *Miscellaneous options* → *Dump DSP firmware*).

## Credits

Based on [Anki](https://github.com/ankitects/anki) by Damien Elmes and the Anki contributors.
Some code is based on [anki-3ds](https://github.com/AlexMolina08/anki-3ds) by AlexMolina08.

## License

Anki-3DS is licensed under the same terms as Anki. See [LICENSE](./LICENSE).
