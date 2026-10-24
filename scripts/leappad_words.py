"""Transcribes lpwords' clips (crates/ribbit-leappad/examples/lpwords.rs): leappad_words.py <dir> [model];
<dir>/words.tsv: spread, region, region name, kind (word/phrase), text.

Pass 1 hears every clip alone. Clips of three or more words are the page's
sentences (and the pictures' remarks). Pass 2 hears each one-word clip again
with its spread's sentences as the prompt, then the spread's single words are
aligned, in order, to the sentences' words, so homophones ("sure"/"shore",
"2"/"to") take the spelling the sentence has."""
import sys, os, wave, re, difflib
import numpy as np
import whisper

d = sys.argv[1]
model = whisper.load_model(sys.argv[2] if len(sys.argv) > 2 else "medium.en", device="cuda")
rows = [l.rstrip("\n").split("\t") for l in open(os.path.join(d, "index.tsv"), encoding="utf-8")]


def audio(path):
    with wave.open(path) as w:
        a = np.frombuffer(w.readframes(w.getnframes()), dtype=np.int16).astype(np.float32) / 32768.0
    a = np.interp(np.arange(0, len(a), 0.5), np.arange(len(a)), a).astype(np.float32)
    a = a / max(1e-4, np.abs(a).max()) * 0.8
    return np.concatenate([np.zeros(4000, np.float32), a, np.zeros(8000, np.float32)])


def hear(a, prompt=None):
    r = model.transcribe(a, language="en", temperature=0.0, condition_on_previous_text=False,
                         fp16=True, initial_prompt=prompt)
    return r["text"].strip()


def tokens(text):
    return re.findall(r"[A-Za-z0-9']+", text)


NUMBERS = {"2": "two", "4": "four", "8": "eight", "1": "one", "0": "oh"}


def sound_key(w):
    w = NUMBERS.get(w, w).lower()
    return re.sub(r"[aeiouyhw]", "", w[:1] + w[1:]) or w


def similar(a, b):
    a, b = a.lower(), b.lower()
    if a == b:
        return 1.0
    return max(difflib.SequenceMatcher(None, a, b).ratio(),
               difflib.SequenceMatcher(None, sound_key(a), sound_key(b)).ratio() * 0.9)


def align(words, vocab):
    """Needleman-Wunsch: words (heard alone, in region order) to vocab (the
    sentences' words). Returns the vocab word for each, or None."""
    n, m = len(words), len(vocab)
    INF = 1e9
    cost = [[INF] * (m + 1) for _ in range(n + 1)]
    back = [[None] * (m + 1) for _ in range(n + 1)]
    cost[0] = [0.15 * j for j in range(m + 1)]
    for i in range(1, n + 1):
        cost[i][0] = cost[i - 1][0] + 1.0
        back[i][0] = "skipword"
        for j in range(1, m + 1):
            opts = [(cost[i - 1][j - 1] + (1 - similar(words[i - 1], vocab[j - 1])) * 2, "match"),
                    (cost[i - 1][j] + 1.0, "skipword"),
                    (cost[i][j - 1] + 0.15, "skipvocab")]
            cost[i][j], back[i][j] = min(opts)
    out, at, i, j = [None] * n, [None] * n, n, m
    while i > 0:
        step = back[i][j]
        if step == "match":
            at[i - 1] = j - 1
            if similar(words[i - 1], vocab[j - 1]) >= 0.5:
                out[i - 1] = vocab[j - 1]
            i, j = i - 1, j - 1
        elif step == "skipword":
            i -= 1
        else:
            j -= 1
    # A word between two that matched neighbouring sentence words is the word
    # between them ("Buh" between "to" and "dock" is "the").
    for i in range(1, n - 1):
        if out[i] is None and out[i - 1] and out[i + 1] and at[i - 1] is not None and at[i + 1] == at[i - 1] + 2:
            out[i] = vocab[at[i - 1] + 1]
    return out


import json
cache_path = os.path.join(d, "heard.json")
cache = json.load(open(cache_path)) if os.path.exists(cache_path) else {}


def unloop(text):
    """Whisper's loops on a long sound: "AHHHHHHHH…" and "Thank you. Thank you.
    Thank you. …" keep one of what repeats."""
    text = re.sub(r"(.)\1{4,}", r"\1\1\1\1", text)
    return re.sub(r"(\b[^.!?]{1,60}?[.!?,]?)(?:\s+\1){2,}", r"\1", text).strip()


def heard(key, a, prompt=None):
    if key not in cache:
        cache[key] = hear(a, prompt)
        json.dump(cache, open(cache_path, "w"), indent=0)
    return unloop(cache[key])


WORDS = {"one": "1", "two": "2", "three": "3", "four": "4", "five": "5", "six": "6", "seven": "7", "eight": "8",
         "nine": "9", "ten": "10", "eleven": "11", "twelve": "12", "thirteen": "13", "fourteen": "14",
         "fifteen": "15", "sixteen": "16", "seventeen": "17", "eighteen": "18", "nineteen": "19", "twenty": "20",
         "thirty": "30"}


def number(w):
    w = w.lower()
    return w if w.isdigit() else WORDS.get(w)


# Words a misheard letter or word can come out as. These are kids' books, so
# none of them is ever printed: a letter said alone ("L" heard as "Hell")
# becomes the letter, anything else is left out.
UNSAFE = {"hell": "L", "ass": "S", "damn": None, "shit": None, "fuck": None, "bitch": None, "crap": None,
          "dick": None, "piss": None, "sex": None, "bastard": None, "slut": None, "whore": None, "cock": None,
          "pussy": None, "tits": None, "boob": None, "boobs": None, "fart": None, "nigger": None, "fag": None}


def safe(text, alone=False):
    """The text with misheard words dropped (or, said alone, made the letter
    they are), or None if nothing is left."""
    words = text.split()
    out = []
    for w in words:
        key = re.sub(r"[^a-z]", "", w.lower())
        if key in UNSAFE:
            if alone and UNSAFE[key]:
                out.append(UNSAFE[key])
            continue
        out.append(w)
    return " ".join(out) or None


# Where each spot is (lplayout <cart> regions > regions.tsv), for reading
# order and the sentence buttons.
PAGE_W, PAGE_H = 945.0, 1395.0
geom = {}
if os.path.exists(os.path.join(d, "regions.tsv")):
    for l in open(os.path.join(d, "regions.tsv"), encoding="utf-8"):
        f = l.rstrip("\n").split("\t")
        geom[(int(f[0]), int(f[1]))] = (int(f[3]), *map(float, f[4:8]))


def is_box(g):
    """A box of one line's height: a printed word, or a sentence's button."""
    return g is not None and g[4] - g[2] < PAGE_H * 0.075


def reading_order(keys):
    """Spots in reading order: left page then right, line by line, left to right."""
    keys = sorted(keys, key=lambda k: (geom[k][0], (geom[k][2] + geom[k][4]) / 2))
    lines, out = [], []
    for k in keys:
        page, x0, y0, x1, y1 = geom[k]
        yc = (y0 + y1) / 2
        if lines and lines[-1][0] == page and abs(yc - lines[-1][1]) < (y1 - y0) / 2:
            lines[-1][2].append(k)
        else:
            lines.append((page, yc, [k]))
    for _, _, ks in lines:
        out += sorted(ks, key=lambda k: geom[k][1])
    return out


def align_at(words, vocab):
    """Needleman-Wunsch of words to vocab, both in reading order: for each
    word, the index of the vocab word it matches (similar enough), or None."""
    n, m = len(words), len(vocab)
    cost = [[0.0] * (m + 1) for _ in range(n + 1)]
    back = [[None] * (m + 1) for _ in range(n + 1)]
    for j in range(1, m + 1):
        cost[0][j], back[0][j] = cost[0][j - 1] + 0.6, "v"
    for i in range(1, n + 1):
        cost[i][0], back[i][0] = cost[i - 1][0] + 0.6, "w"
        for j in range(1, m + 1):
            cost[i][j], back[i][j] = min((cost[i - 1][j - 1] + (1 - similar(words[i - 1], vocab[j - 1])) * 1.5, "m"),
                                         (cost[i - 1][j] + 0.6, "w"), (cost[i][j - 1] + 0.6, "v"))
    out, i, j = [None] * n, n, m
    while i > 0 and j > 0:
        step = back[i][j]
        if step == "m":
            if words[i - 1] and similar(words[i - 1], vocab[j - 1]) >= 0.5:
                out[i - 1] = j - 1
            i, j = i - 1, j - 1
        elif step == "w":
            i -= 1
        else:
            j -= 1
    # Between two matched words, as many boxes as sentence words: they are
    # those words in order (also before the first match and after the last,
    # where the counts agree).
    anchors = [(i, a) for i, a in enumerate(out) if a is not None]
    for (i, a), (k, b) in zip(anchors, anchors[1:]):
        if k - i == b - a:
            for t in range(1, k - i):
                out[i + t] = a + t
    if anchors:
        i, a = anchors[0]
        if a >= i:
            for t in range(i):
                out[t] = a - i + t
        k, b = anchors[-1]
        if m - b >= n - k:
            for t in range(k + 1, n):
                out[t] = b + t - k
    return out


by_spread = {}
for r in rows:
    spread, region, name = int(r[0]), int(r[1]), r[2]
    path = os.path.join(d, f"{spread}-{region}.wav")
    if not name.startswith("GSwitch"):
        by_spread.setdefault(spread, []).append((region, name, path if os.path.exists(path) else None))

# How many spreads each spot's shape is on (the stop sign is on most).
common = {}
for spread_rows in by_spread.values():
    for n in {n for _, n, _ in spread_rows}:
        common[n] = common.get(n, 0) + 1

out = open(os.path.join(d, "words.tsv"), "w", encoding="utf-8")
for spread in sorted(by_spread):
    names = {r: n for r, n, _ in by_spread[spread]}
    clips = [(r, n, p, audio(p)) for r, n, p in by_spread[spread] if p]
    first = {r: heard(f"{spread}-{r}", a) for r, _, _, a in clips}
    sentences = [first[r] for r, _, _, _ in clips if len(tokens(first[r])) >= 3]
    prompt = " ".join(sentences)[-800:] or None
    second = {}
    for r, _, _, a in clips:
        if len(tokens(first[r])) <= 2:
            t = tokens(heard(f"{spread}-{r}-p", a, prompt))
            second[r] = t[0] if len(t) == 1 else (" ".join(tokens(first[r])) or "")
    # The printed text: word boxes, and the sentence buttons (a box that says
    # a whole sentence, at the start of a line of word boxes).
    boxes = [r for r in names if is_box(geom.get((spread, r)))]
    def button(r):
        if r not in first or len(tokens(first[r])) < 3:
            return False
        page, x0, y0, x1, y1 = geom[(spread, r)]
        # Roughly square, and first on its line (word boxes that read their
        # whole sentence in some mode are wider, with words before them).
        if x1 - x0 > 1.5 * (y1 - y0):
            return False
        same_line = lambda b: geom[(spread, b)][0] == page and abs((geom[(spread, b)][2] + geom[(spread, b)][4]) / 2 - (y0 + y1) / 2) < (y1 - y0) / 2
        if any(same_line(b) and 0 <= x0 - geom[(spread, b)][3] < 40 for b in boxes if b != r):
            return False
        return any(geom[(spread, b)][0] == page and abs((geom[(spread, b)][2] + geom[(spread, b)][4]) / 2 - (y0 + y1) / 2) < (y1 - y0) / 2
                   and 0 <= geom[(spread, b)][1] - x1 < 40 for b in boxes if b != r)
    buttons = [r for r in reading_order([(spread, r) for r in boxes]) if button(r[1])]
    # Page numbers: a spot in a bottom corner that says only a number.
    def said_number(r):
        g = geom.get((spread, r))
        if r not in first or g is None:
            return None
        page, x0, y0, x1, y1 = g
        corner = y0 > PAGE_H * 0.8 and ((page == 0 and x0 < 160) or (page == 1 and x1 > PAGE_W - 160))
        t = tokens(first[r])
        return number(t[0]) if corner and len(t) == 1 else None
    numbers = {r: said_number(r) for r in names if said_number(r)}
    # The stop sign: the spot just above the page number, the same shape on
    # most spreads (Cars' "Touch 32774", Tigger's VSwitch 33585 ...).
    def is_stop(r):
        g = geom.get((spread, r))
        if g is None or common.get(names[r], 0) * 2 < len(by_spread):
            return False
        page, x0, y0, x1, y1 = g
        if not 0.6 < (x1 - x0) / max(1.0, y1 - y0) < 1.7 or x1 - x0 > 170:
            return False
        for n in numbers:
            npage, nx0, ny0, nx1, ny1 = geom[(spread, n)]
            if npage == page and x0 < nx1 and x1 > nx0 and 0 <= ny0 - y1 < 80:
                return True
        return False
    stops = {r for r in names if is_stop(r)}
    numbers = {r: v for r, v in numbers.items() if r not in stops}
    words_at = [k[1] for k in reading_order([(spread, r) for r in boxes]) if k not in buttons and k[1] not in numbers and k[1] not in stops]
    fixed = {}
    if buttons:
        vocab = [t for _, r in buttons for t in tokens(first[r])]
        said = [second.get(r) or (tokens(first[r])[0] if r in first and tokens(first[r]) else "") for r in words_at]
        for r, at in zip(words_at, align_at(said, vocab)):
            if at is not None:
                fixed[r] = vocab[at]
    # The rest (no buttons, or words outside their sentences): the spread's
    # sentences, in spot order.
    single = [r for r, _, _, _ in clips if r in second and r not in fixed and r not in numbers and r not in stops]
    vocab = [t for s_ in sentences for t in tokens(s_)]
    for r, a in zip(single, align([second[r] for r in single], vocab) if vocab else [None] * len(single)):
        if a:
            fixed[r] = a
    for r in sorted(names):
        n = names[r]
        if r in stops:
            text, kind = "stop", "stop"
        elif (spread, r) in buttons:
            text, kind = safe(first[r]), "sentence"
        elif r in numbers:
            text, kind = numbers[r], "word"
        elif r in fixed:
            text, kind = safe(fixed[r], alone=True), "word"
        elif r in second:
            # A page number, or a short remark (a sound effect, "Wow!"),
            # which is not printed.
            num = number(second[r])
            if num:
                text, kind = num, "word"
            elif second[r].lower() in ("you", "thank you", "") or first[r].lower().strip(".!? ") in ("you", "thank you"):
                continue
            else:
                text, kind = safe(" ".join(tokens(first[r])) or second[r], alone=True), "phrase"
        elif r in first:
            text, kind = safe(first[r]), "phrase"
        else:
            continue
        if text:
            out.write(f"{spread}\t{r}\t{n}\t{kind}\t{text}\n")
    out.flush()
    print(f"spread {spread}: {len(buttons)} sentence buttons, {sum(r in fixed for r in words_at)}/{len(words_at)} word boxes", flush=True)
