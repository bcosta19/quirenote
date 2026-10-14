# Rejected partial campaign

These 25 samples are exploratory and are excluded from the final comparison.
The campaign was interrupted before its three repetitions completed.

- `/tmp` was a tmpfs at 80% occupancy. Two inactive build/cache directories were
  subsequently moved to disk with the user's permission, changing the memory conditions.
- The native navigation screenshot shows a blank PDF and inconsistent page state
  after returning from an EPUB. This failed the visual verification even though
  the persisted session check passed.
- The native reader was then corrected and two regressions reproduced both bugs
  before the fix and passed afterward.

The accepted campaign uses a freshly built native binary. Do not merge these
samples into its medians. Source hashes, binary hashes and timing parameters of
this partial run remain in `metadata.json`.
