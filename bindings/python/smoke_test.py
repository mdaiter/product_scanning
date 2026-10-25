"""Smoke test for the UniFFI Python bindings. Every plugin here is implemented in Python, which is
exactly what a Swift or Kotlin app does with CoreML / ML Kit: the Rust pipeline calls back into the host.

Run via scripts/python_smoke.sh (builds the library, generates the bindings, copies the .so next to them).
"""
import asyncio
import sys
import threading
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent / "generated"))
import shelfscan as ss  # noqa: E402

W, H = 160, 120
TABLE = (196, 164, 120)
BOXES = [  # (x0, y0, x1, y1, colour)
    (10, 30, 40, 80, (200, 30, 40)),
    (65, 35, 95, 90, (40, 80, 200)),
    (115, 25, 148, 78, (50, 160, 70)),
]


def make_frame(t_ms: int, jitter: int = 0) -> "ss.Frame":
    buf = bytearray(bytes(TABLE) * (W * H))
    for x0, y0, x1, y1, c in BOXES:
        for y in range(y0 + jitter, y1 + jitter):
            row = (y * W + x0) * 3
            buf[row : row + (x1 - x0) * 3] = bytes(c) * (x1 - x0)
    return ss.Frame(width=W, height=H, format=ss.PixelFormat.RGB8, data=bytes(buf), timestamp_ms=t_ms)


class PyDetector(ss.Detector):
    """Column-projection detector for side-by-side rectangles. Runs in Python, called from Rust."""

    def __init__(self):
        self.calls = 0

    def detect(self, frame):
        self.calls += 1
        d = frame.data
        w, h = frame.width, frame.height
        bg = d[0:3]

        def is_bg(i):
            return abs(d[i] - bg[0]) + abs(d[i + 1] - bg[1]) + abs(d[i + 2] - bg[2]) < 40

        cols = [any(not is_bg((y * w + x) * 3) for y in range(0, h, 2)) for x in range(w)]
        out, x = [], 0
        while x < w:
            if cols[x]:
                x0 = x
                while x < w and cols[x]:
                    x += 1
                ys = [y for y in range(h) if any(not is_bg((y * w + xx) * 3) for xx in range(x0, x, 3))]
                if ys and x - x0 > 3:
                    out.append(ss.Detection(bbox=ss.BBox(x=x0 / w, y=ys[0] / h, w=(x - x0) / w, h=(ys[-1] - ys[0] + 1) / h), score=0.9, label="product"))
            else:
                x += 1
        return out


class PyGuesser(ss.ProductGuesser):
    def __init__(self):
        self.calls = 0

    def guess(self, request):
        self.calls += 1
        if self.calls == 1:
            raise RuntimeError("model not loaded yet")  # arrives in Rust as a Backend error, not a crash
        colour = request.dominant_colors[0] if request.dominant_colors else "mystery"
        return ss.ProductGuess(title=f"Python {colour} thing", brand=None, category=None, attributes=[], confidence=0.6)


class PyWriter(ss.ListingWriter):
    def write(self, item, style_prompt):
        assert "plain English" in style_prompt
        return ss.Listing(
            id="", source_item_id="", title=f"{item.guess.title} (py)", description_html="<p>from python</p>", vendor="", product_type="",
            tags=["py"], status=ss.ListingStatus.ACTIVE, options=[], variants=[], images=[], seo_title=None, seo_description=None,
            confidence=item.guess.confidence, published=False,
        )


class PyStorefront(ss.Storefront):
    def __init__(self):
        self.seen = []

    def name(self):
        return "python"

    def publish(self, listings):
        self.seen += listings
        return [ss.PublishReceipt(listing_id=l.id, ok=True, remote_id="py-" + l.id, url=None, message="stored") for l in listings]


class PyListener(ss.EventListener):
    def __init__(self):
        self.events, self.lock = [], threading.Lock()

    def on_event(self, event):
        with self.lock:
            self.events.append(event)

    def count(self, cls):
        with self.lock:
            return sum(isinstance(e, cls) for e in self.events)


async def main():
    cfg = ss.EngineConfig(default_mode=ss.ScanMode.BARCODE_FIRST, analysis_fps=20.0, novelty_threshold=0.0, blur_gate_ratio=0.0)
    det, guesser, listener, store = PyDetector(), PyGuesser(), PyListener(), PyStorefront()
    engine = (
        ss.EngineBuilder(cfg).detector(det).guesser(guesser).listing_writer(PyWriter()).listener(listener).build()
    )

    for i in range(10):
        t = 1000 + i * 100
        for _ in range(2000):
            if engine.wants_frame(t):
                break
            time.sleep(0.002)
        assert engine.push_frame(make_frame(t, jitter=i % 2)), "frame refused"

    summary = await engine.flush()
    assert summary.items == 3, summary
    assert det.calls >= 3

    items = await engine.catalog(False)
    assert len(items) == 3
    failed = [i for i in items if i.state.is_UNKNOWN()]
    ok = [i for i in items if i.state.is_GUESSED()]
    assert len(failed) == 1 and len(ok) == 2, [str(i.state) for i in items]
    assert all(i.guess.title.startswith("Python ") for i in ok)

    # a Python exception inside a plugin is a Warning, and the item can ask again
    deadline = time.time() + 5
    while listener.count(ss.Event.WARNING) == 0 and time.time() < deadline:
        time.sleep(0.01)
    assert listener.count(ss.Event.WARNING) >= 1
    retried = await engine.reject_item(failed[0].id, None)
    assert retried is not None
    for _ in range(500):
        again = await engine.item(failed[0].id)
        if again.state.is_GUESSED():
            break
        await asyncio.sleep(0.01)
    assert again.state.is_GUESSED()

    # typed errors cross the boundary as typed exceptions
    try:
        await engine.confirm_item("item-999")
        raise AssertionError("expected an error")
    except ss.ShelfError.InvalidInput:
        pass
    try:
        await engine.submit_barcode(ok[0].id, "1234567890123")  # bad check digit
        raise AssertionError("expected an error")
    except ss.ShelfError.InvalidInput:
        pass

    for it in await engine.catalog(False):
        await engine.confirm_item(it.id)
    confirmed = await engine.submit_barcode(ok[0].id, "4006381333931")
    assert confirmed.state.is_CONFIRMED() and confirmed.gtin == "4006381333931"

    listings = await engine.generate_listings(False)
    assert len(listings) == 3
    assert all(l.status == ss.ListingStatus.DRAFT for l in listings), "writer asked for ACTIVE; engine must force DRAFT"
    assert any(l.variants[0].sku == "4006381333931" for l in listings)

    receipts = await engine.publish([l.id for l in listings], store)
    assert len(receipts) == 3 and all(r.ok for r in receipts)
    assert all(l.status == ss.ListingStatus.DRAFT for l in store.seen)

    assert listener.count(ss.Event.OVERLAY) >= 1
    assert listener.count(ss.Event.ITEM_DISCOVERED) == 3
    stats = engine.stats()
    assert stats.frames_in == 10, stats

    engine.shutdown()
    try:
        await engine.flush()
        raise AssertionError("expected Closed")
    except ss.ShelfError.Closed:
        pass
    print("python smoke test OK:", stats)

    await builtins_check()


async def builtins_check():
    """Built-in Rust plugins can be handed to the engine from foreign code, no Python plugin needed."""
    import csv
    import tempfile

    out = Path(tempfile.mkdtemp()) / "products.csv"
    cfg = ss.EngineConfig(default_mode=ss.ScanMode.DESCRIBE_FIRST, analysis_fps=20.0, novelty_threshold=0.0, blur_gate_ratio=0.0)
    engine = (
        ss.EngineBuilder(cfg)
        .detector(ss.BlobDetector())
        .embedder(ss.ColorEmbedder())
        .guesser(ss.PlaceholderGuesser())
        .listing_writer(ss.TemplateWriter())
        .build()
    )
    for i in range(10):
        t = 1000 + i * 100
        for _ in range(2000):
            if engine.wants_frame(t):
                break
            time.sleep(0.002)
        engine.push_frame(make_frame(t, jitter=i % 2))
    await engine.flush()
    items = await engine.catalog(False)
    assert len(items) == 3, [i.id for i in items]
    for it in items:
        await engine.confirm_item(it.id)
    listings = await engine.generate_listings(False)
    receipts = await engine.publish([l.id for l in listings], ss.CsvStorefront(str(out), False))
    assert len(receipts) == 3 and all(r.ok for r in receipts), receipts
    rows = list(csv.DictReader(out.open(newline="")))
    assert len(rows) == 3 and all(r["Status"] == "draft" and r["Published on online store"] == "false" for r in rows), rows
    engine.shutdown()
    print("python built-in plugins OK:", out.name, len(rows), "draft rows")


asyncio.run(main())
