"""Live microphone levels for the recording indicator.

The recorder streams its WAV to disk as it captures, so the meter tails that
file instead of opening the microphone a second time. Each tick it reads what
was written since the last one, measures a dozen frequency bands over the
newest samples with a small FFT, and smooths them like a hardware
equalizer: instant attack, slow decay. ``bars`` turns levels into rows of
block characters; every front end draws those, so the indicator looks the same
in the composer, the popup, and the Herdr client's mode bar.
"""

import cmath
import math
import struct
import time

SAMPLE_RATE = 16000
FPS = 15
BANDS = 12
WINDOW = 1024  # samples analyzed per tick (64 ms at 16 kHz)
DECAY = 0.72  # per tick, when a band gets quieter
FLOOR_RISE = 0.004  # per tick: the noise floor re-adapts within a few seconds
FLOOR_DB, RANGE_DB = -54.0, 46.0  # a band at -54 dBFS is empty, at -8 dBFS is full
QUIET_PEAK = 0.005  # below this (about -46 dBFS) the microphone is considered silent
QUIET_AFTER = 3.0  # seconds of silence before the UIs mention it
BLOCKS = " ▁▂▃▄▅▆▇█"
_HEADER_LIMIT = 4096


def band_edges(count=BANDS, low=80.0, high=4000.0):
    """``count + 1`` log-spaced edges across the voice range."""
    ratio = (high / low) ** (1 / count)
    return [low * ratio ** index for index in range(count + 1)]


def band_frequencies(count=BANDS, low=80.0, high=4000.0):
    """The geometric center of each band."""
    edges = band_edges(count, low, high)
    return [math.sqrt(a * b) for a, b in zip(edges, edges[1:])]


def _plan(size):
    """Bit-reversal order and per-stage twiddles for an iterative radix-2 FFT."""
    bits = size.bit_length() - 1
    order = [int(format(index, "0%db" % bits)[::-1], 2) for index in range(size)]
    stages, span = [], 2
    while span <= size:
        half = span // 2
        stages.append((span, half, [cmath.exp(-2j * math.pi * k / span) for k in range(half)]))
        span *= 2
    return order, stages


def _fft(values, order, stages):
    data = [values[index] for index in order]
    for span, half, twiddles in stages:
        for start in range(0, len(data), span):
            for k in range(half):
                i, j = start + k, start + k + half
                t = twiddles[k] * data[j]
                data[j] = data[i] - t
                data[i] = data[i] + t
    return data


class Meter:
    """Band levels (0..1) for a WAV file that is still being written."""

    def __init__(self, path, bands=BANDS):
        self.path = path
        self.levels = [0.0] * bands
        self.floors = [1.0] * bands  # the quietest recent level per band: room noise, subtracted from the display
        self.peak = 0.0
        self.offset = None  # byte offset of the next unread sample; None until the data chunk is found
        self.tail = b""
        self.started = self.loud_at = time.monotonic()
        self._order, self._stages = _plan(WINDOW)
        self._window = [0.5 - 0.5 * math.cos(2 * math.pi * index / (WINDOW - 1)) for index in range(WINDOW)]
        bins = [max(1, round(edge * WINDOW / SAMPLE_RATE)) for edge in band_edges(bands)]
        self._bins = [(start, max(start + 1, stop)) for start, stop in zip(bins, bins[1:])]
        self._full_scale = (32768 * WINDOW / 4) ** 2  # power in the main bin of a full-scale sine under a Hann window

    # ----- input ---------------------------------------------------------------

    def _locate_data(self, stream):
        """Walk the RIFF chunks once to find where samples begin; None while the header is incomplete."""
        head = stream.read(_HEADER_LIMIT)
        if len(head) < 12 or head[:4] != b"RIFF" or head[8:12] != b"WAVE":
            return None
        position = 12
        while position + 8 <= len(head):
            chunk, size = head[position:position + 4], struct.unpack("<I", head[position + 4:position + 8])[0]
            if chunk == b"data":
                return position + 8
            position += 8 + size + (size & 1)
        return None

    def _read_new(self):
        try:
            with open(self.path, "rb") as stream:
                if self.offset is None:
                    self.offset = self._locate_data(stream)
                    if self.offset is None:
                        return b""
                stream.seek(self.offset)
                data = stream.read()
        except OSError:
            return b""
        self.offset += len(data)
        return data

    # ----- analysis -------------------------------------------------------------

    def _powers(self, samples):
        """Summed spectral power per band over the newest window."""
        spectrum = _fft([sample * weight for sample, weight in zip(samples, self._window)], self._order, self._stages)
        power = [abs(value) ** 2 for value in spectrum[:self._bins[-1][1]]]
        return [sum(power[start:stop]) for start, stop in self._bins]

    def update(self):
        """Fold in whatever the recorder wrote since the last call. Returns the levels."""
        data = self._read_new()
        buffer = self.tail + data
        if len(buffer) & 1:
            buffer = buffer[:-1]
        self.tail = buffer[-WINDOW * 2:]
        if not data:
            self.levels = [level * DECAY for level in self.levels]
            return self.levels
        count = len(data) // 2
        if count:
            fresh = struct.unpack("<%dh" % count, data[:count * 2])
            self.peak = max(abs(sample) for sample in fresh) / 32768
            if self.peak > QUIET_PEAK:
                self.loud_at = time.monotonic()
        if len(self.tail) < WINDOW * 2:
            return self.levels
        samples = struct.unpack("<%dh" % WINDOW, self.tail)
        for index, power in enumerate(self._powers(samples)):
            decibels = 10 * math.log10(power / self._full_scale) if power > 0 else -120.0
            raw = min(1.0, max(0.0, (decibels - FLOOR_DB) / RANGE_DB))
            # Room noise would otherwise keep every bar half lit. The floor drops at once and
            # creeps back up, so a quieter room lowers it and a steady hum never looks like speech.
            floor = min(self.floors[index] + FLOOR_RISE, raw)
            self.floors[index] = floor
            level = (raw - floor) / (1 - floor) if floor < 1 else 0.0
            self.levels[index] = level if level >= self.levels[index] else self.levels[index] * DECAY
        return self.levels

    def quiet(self):
        """True once nothing audible has arrived for a while: a muted or missing microphone."""
        return time.monotonic() - self.loud_at > QUIET_AFTER


def synthetic(elapsed, bands=BANDS):
    """Lively made-up levels for previews and tests, shaped like speech: louder low bands, bursts over time."""
    burst = 0.55 + 0.45 * math.sin(elapsed * 2.3) * math.sin(elapsed * 0.7 + 1)
    return [min(1.0, max(0.0, burst * (1.05 - index / (bands * 1.4)) * (0.55 + 0.45 * math.sin(elapsed * 9.1 + index * 1.7)))) for index in range(bands)]


# ----- rendering --------------------------------------------------------------

def columns(levels, width):
    """Spread the bands over ``width`` cells; None marks a gap column between wide bars."""
    if width <= 0 or not levels:
        return []
    per_band = max(1, width // len(levels))
    result = []
    for level in levels:
        if len(result) + per_band > width:
            break
        result.extend([level] * (per_band - 1 if per_band > 1 else 1))
        if per_band > 1:
            result.append(None)
    if result and result[-1] is None:
        result.pop()
    return result


def shade(level):
    """0 low, 1 mid, 2 hot: the color a single-row bar or a row of a tall meter should take."""
    return 2 if level > 0.85 else 1 if level > 0.6 else 0


def bars(levels, width, rows=1):
    """Rows of (text, shade) runs, top row first, ready for a text drawing routine.

    A one-row meter colors each bar by its own height. A taller meter colors by
    row like a hardware equalizer: green at the bottom, red at the top.
    """
    cells = columns(levels, width)
    rendered = []
    for row in range(rows):
        below = (rows - 1 - row) * 8
        runs = []
        for level in cells:
            if level is None:
                character, tone = " ", 0
            else:
                fill = min(8, max(0, round(level * rows * 8) - below))
                if row == rows - 1:
                    fill = max(1, fill)
                character = BLOCKS[fill]
                tone = 0 if character == " " else shade(level) if rows == 1 else min(2, 3 * (rows - 1 - row) // rows)
            if runs and runs[-1][1] == tone:
                runs[-1] = runs[-1][0] + character, tone
            else:
                runs.append((character, tone))
        rendered.append(runs)
    return rendered


def line(levels, width):
    """A one-row meter as plain text, for places that cannot color runs."""
    return "".join(text for text, _ in bars(levels, width)[0])
