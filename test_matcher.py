import unittest

from screen_click_gui import (
    _norm,
    _offset_words,
    build_text_candidates,
    merge_ocr_words,
    resolve_selector_matches,
)


def word(text, x, line=0, index=0):
    return {
        "text": text,
        "x": x,
        "y": 10,
        "w": len(text) * 8,
        "h": 14,
        "line": line,
        "word": index,
    }


class MatcherTests(unittest.TestCase):
    def test_normalization_ignores_spaces_and_punctuation(self):
        self.assertEqual(_norm("Open File!"), "openfile")
        self.assertEqual(_norm("open f"), "openf")

    def test_phrase_candidate_spans_adjacent_words(self):
        candidates = build_text_candidates([
            word("Open", 0, index=0),
            word("File", 50, index=1),
        ])

        matches, _, _ = resolve_selector_matches(_norm("open f"), candidates)

        self.assertEqual(len(matches), 1)
        self.assertEqual(matches[0]["text"], "Open File")
        self.assertEqual(matches[0]["n"], "openfile")

    def test_text_match_wins_over_selector_suffix(self):
        candidates = build_text_candidates([
            word("Open", 0, index=0),
            word("File", 50, index=1),
        ])
        matches, ctx, suffix = resolve_selector_matches(_norm("open"), candidates)
        self.assertEqual(suffix, "")
        self.assertEqual(matches[0]["text"], "Open")

        matches, _, suffix = resolve_selector_matches(_norm("openf"), candidates, ctx)

        self.assertEqual(suffix, "")
        self.assertEqual(matches[0]["text"], "Open File")

    def test_middle_of_word_search_matches_visible_text(self):
        candidates = build_text_candidates([
            word("Settings", 0, index=0),
            word("Switches", 90, index=1),
        ])

        matches, _, suffix = resolve_selector_matches(_norm("ttin"), candidates)

        self.assertEqual(suffix, "")
        self.assertEqual([m["text"] for m in matches], ["Settings"])

    def test_contains_search_suppresses_larger_overlapping_phrases(self):
        candidates = build_text_candidates([
            word("Main", 0, index=0),
            word("app", 42, index=1),
            word("screen_click_gui.py", 82, index=2),
        ])

        matches, _, suffix = resolve_selector_matches(_norm("app"), candidates)

        self.assertEqual(suffix, "")
        self.assertEqual([m["text"] for m in matches], ["app"])

    def test_selector_suffix_disqualifies_nonmatching_highlights(self):
        candidates = build_text_candidates([
            word("Alpha", 0, index=0),
            word("Alpine", 80, index=1),
            word("Alt", 160, index=2),
        ])
        matches, ctx, _ = resolve_selector_matches(_norm("al"), candidates)
        self.assertEqual(len(matches), 3)
        chosen_hint = matches[1]["hint"]

        narrowed, _, suffix = resolve_selector_matches(
            _norm("al" + chosen_hint[0]), candidates, ctx)

        self.assertEqual(suffix, chosen_hint[0])
        self.assertTrue(0 < len(narrowed) < len(matches))
        self.assertIn("Alpine", {m["text"] for m in narrowed})

    def test_offset_words_moves_monitor_coordinates_into_base_region(self):
        words = [word("Left", 10, line=2, index=0)]
        monitor = {"left": -1080, "top": -256, "width": 1080, "height": 1920}
        base_region = (-1080, -267, 7280, 1931)

        moved, next_line = _offset_words(words, monitor, base_region, 10)

        self.assertEqual(moved[0]["x"], 10)
        self.assertEqual(moved[0]["y"], 21)
        self.assertEqual(moved[0]["line"], 12)
        self.assertEqual(next_line, 13)

    def test_merge_ocr_words_keeps_primary_and_adds_new_readings(self):
        primary = [word("Switch", 100, index=0), word("Settings", 200, index=1)]
        extra = [
            word("Switch", 102, index=0),  # duplicate same text/location
            word("Swltch", 102, index=1),  # different OCR reading, keep it
            word("Search", 300, index=2),  # new word
        ]

        merged = merge_ocr_words(primary, extra)

        self.assertEqual(
            [w["text"] for w in merged],
            ["Switch", "Settings", "Swltch", "Search"],
        )


if __name__ == "__main__":
    unittest.main()
