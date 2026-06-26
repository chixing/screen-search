import unittest

from screen_click_gui import (
    _norm,
    build_text_candidates,
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

    def test_text_prefix_wins_over_selector_suffix(self):
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


if __name__ == "__main__":
    unittest.main()
