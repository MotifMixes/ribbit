# LeapPad virtual books

A LeapPad book is paper: it has no electronics and no codes. The pad senses where the pen is, and the
cartridge decides what that spot means on the page the child said is open, by touching the page's green GO spot
(docs/leappad.md, "How the toy is used"). So Ribbit's virtual book is **pages to look at plus positions**.

* **Touching a virtual page only moves the emulated pen.** Every answer comes from the cartridge's own software:
  the region hit, the view change, the sound.
* **Turning a page in Ribbit** (◀ ▶) only changes the picture. As on the real pad, touch the spread's GO spot so
  the cartridge knows it is open.

## Files

A book is a TOML file (UTF-8). Page pictures are PNG files next to it.

* **Opening a book:** Emulation > LeapPad (1999) > Open book…, or "Book…" on the pad's bar.
* **Remembered:** Ribbit keeps the book opened with each cartridge.
* **Opened automatically:** a book with the cartridge's name and `.toml` (e.g. `500-00406.toml` beside
  `500-00406.bin`) opens when the cartridge starts.
* **"Test book"** opens Ribbit's own test book (`crates/ribbit-leappad/books/pen-test.toml`).
* **Drawn from the cartridge:** with no book file, Ribbit draws the book from the cartridge's own layout
  ("From cartridge" on the pad's bar): blank pages with the outline of every spot the cartridge answers, and
  its GO spots. A spot lights up under the pen, and the debug panel names it (`Simple 32867`: the clickable
  type and shape number the cartridge prints when touched). This works for cartridges with the "Prebuilt TnT"
  app and for LeapPad Classic cartridges (19 of the 20 dumps tried; docs/leappad.md, "The cartridge's touch
  layout" and "The Classic app's touch layout"). On Classic pages the outlines are mode 1's (whole words). The
  other modes (sounding out …) answer inside the same words.

```toml
format = 1                       # this version of the format
title = "Tutter's Tiny Trip"
cartridge = "500-00406"          # the cartridge's part number (or cartridge_sha1 = "...")

[[spread]]                       # one per opened pair of pages, in order
name = "Pages 2-3"
left = "pages/p02.png"           # optional page pictures, relative to this file
right = "pages/p03.png"
go = { page = "left", x = 100, y = 100 }   # the printed GO spot

  [[spread.region]]              # optional named areas (overlays, tests, notes)
  name = "Tutter"
  page = "right"
  rect = [300, 420, 520, 700]    # x0, y0, x1, y1
  outline = [[310, 700], [300, 450], [420, 420], [520, 690]]   # optional polygon inside rect
  note = "says Tutter's name"
```

### Coordinates

* **Units:** the cartridge's own page coordinates, the numbers its software prints in its debug log for a touch
  (`(0: 495, 623)[Touch]`).
* **Range:** x runs across a page from 0 to 945, y down from 0 to 1395, the same on the left and right pages.
* **Shape:** a page is portrait, 945 × 1395 (aspect about 0.68). Pictures are stretched to that shape.
* **Where the range comes from:** the calibration points in the cartridge (docs/leappad.md, "Pen position"),
  which put the edges of the sensed area at about −22 and 922 across and 1372 down.
* **The real paper's edges** relative to these numbers are UNKNOWN until a real book is measured.

### Checks

When a book is opened, Ribbit reports:

* **errors** (the book does not open): an unsupported `format`; no title; no spreads; a GO spot off the page; a
  region whose rectangle is empty, inverted or off the page; an outline of fewer than 3 points or with a point
  off the page; a file that is not valid TOML.
* **warnings** (the book opens): a page picture that is missing (a plain page is shown); a spread without a GO
  spot; two regions with the same name; no `cartridge` given; a book made for a different cartridge from the one
  in the pad.

Warnings show in the debug panel (Debug on the pad's bar).

## Making a book for a real cartridge

The pictures must come from your own copy of the book (photographs or scans). Ribbit includes none, and none may
be added to its repository.

* **Start from the cartridge's layout:** `lplayout <cartridge> book <file.toml>` (an example program in
  `crates/ribbit-leappad`) writes the book Ribbit draws from the cartridge as a book file: every spread with its
  GO spot and every spot as a region, outlines included. Add `left = "…"` and `right = "…"` picture lines to
  each spread, and the spots line up with the pictures if the pictures are cropped to the page area.
* **Placing the pictures:** where the real paper's edges are in the cartridge's coordinates is UNKNOWN (see
  "Coordinates"). Crop a page scan so that the GO tabs and a few outlined spots line up with the printed ones.
* **Checking:** the debug panel shows where the cartridge felt each touch and which region that is.

## The test book

`crates/ribbit-leappad/books/pen-test.toml` is an original fixture, not a LeapFrog book:

* three spreads of numbered targets on both pages, and a GO spot near the top-left corner (where Tutter's Tiny
  Trip answers with a page-pointer touch);
* `cargo test -p ribbit-leappad --release -- --ignored`, with `RIBBIT_LEAPPAD_BIOS` and `RIBBIT_LEAPPAD_CART`
  set, presses each target of the first spread on the real cartridge and checks where the cartridge felt it.
  The worst error with Tutter's Tiny Trip is 0.63% of the page.
