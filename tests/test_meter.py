import math
import os
import struct
import tempfile
import unittest
from unittest.mock import patch

from inbox import meter


def wav_header(extra_chunk=b""):
    return b"RIFF" + struct.pack("<I", 0) + b"WAVE" + b"fmt " + struct.pack("<IHHIIHH", 16, 1, 1, 16000, 32000, 2, 16) + extra_chunk + b"data" + struct.pack("<I", 0)


def tone(frequency, amplitude, samples=meter.WINDOW):
    return struct.pack("<%dh" % samples, *(int(amplitude * math.sin(2 * math.pi * frequency * index / meter.SAMPLE_RATE)) for index in range(samples)))


class MeterTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.path = os.path.join(self.directory.name, "live.wav")

    def tearDown(self):
        self.directory.cleanup()

    def test_band_centers_climb_the_voice_range(self):
        edges, centers = meter.band_edges(), meter.band_frequencies()
        self.assertEqual((len(edges), len(centers)), (meter.BANDS + 1, meter.BANDS))
        self.assertAlmostEqual(edges[0], 80)
        self.assertAlmostEqual(edges[-1], 4000)
        self.assertTrue(all(a < c < b for a, c, b in zip(edges, centers, edges[1:])))

    def test_levels_follow_the_file_as_the_recorder_writes_it(self):
        live = meter.Meter(self.path)
        self.assertEqual(live.update(), [0.0] * meter.BANDS)  # nothing written yet
        with open(self.path, "wb") as stream:
            stream.write(wav_header(b"LIST" + struct.pack("<I", 4) + b"INFO"))
            stream.write(tone(1000, 0))  # a silent first tick sets the noise floor
            stream.flush()
            live.update()
            stream.write(tone(1000, 12000))
            stream.flush()
            levels = list(live.update())
        self.assertEqual(live.offset, 44 + 12 + meter.WINDOW * 4)
        loudest = max(range(meter.BANDS), key=levels.__getitem__)
        self.assertTrue(meter.band_edges()[loudest] <= 1000 <= meter.band_edges()[loudest + 1])
        self.assertGreater(levels[loudest], 0.8)
        self.assertLess(min(levels), 0.3)
        self.assertAlmostEqual(live.peak, 12000 / 32768, places=2)
        quieter = live.update()  # nothing new: every band decays
        self.assertTrue(all(after < before or before == 0 for after, before in zip(quieter, levels)))

    def test_steady_room_noise_settles_near_the_floor(self):
        live = meter.Meter(self.path)
        with open(self.path, "wb") as stream:
            stream.write(wav_header())
            for _ in range(6):
                stream.write(tone(300, 3000))
                stream.flush()
                levels = list(live.update())
        self.assertLess(max(levels), 0.15)

    def test_quiet_means_no_audible_sample_for_a_while(self):
        live = meter.Meter(self.path)
        with open(self.path, "wb") as stream:
            stream.write(wav_header() + tone(500, 0))
        with patch("inbox.meter.time.monotonic", side_effect=[100.0, 104.0, 104.5, 105.0]):
            live = meter.Meter(self.path)
            live.update()
            self.assertTrue(live.quiet())
            with open(self.path, "ab") as stream:
                stream.write(tone(500, 2000))
            live.update()
            self.assertFalse(live.quiet())

    def test_bars_render_one_row_by_level_and_tall_meters_by_height(self):
        levels = [0.0, 0.5, 1.0]
        self.assertEqual(meter.bars(levels, 3), [[("▁▄", 0), ("█", 2)]])
        self.assertEqual(meter.line(levels, 3), "▁▄█")
        self.assertEqual(meter.line(levels, 6), "▁ ▄ █")
        rows = meter.bars(levels, 3, rows=3)
        self.assertEqual(rows, [[("  ", 0), ("█", 2)], [(" ", 0), ("▄█", 1)], [("▁██", 0)]])
        self.assertEqual(meter.bars(levels, 0), [[]])

    def test_synthetic_levels_stay_in_range(self):
        for elapsed in (0, 0.3, 2.9, 17.1):
            levels = meter.synthetic(elapsed)
            self.assertEqual(len(levels), meter.BANDS)
            self.assertTrue(all(0 <= level <= 1 for level in levels))


if __name__ == "__main__":
    unittest.main()
