# tools

Development helpers.

`fake_stream.py` serves local MPEG-TS test streams over HTTP, so playback
can be exercised without a live account and without competing for a
provider's connections. See `python3 tools/fake_stream.py --help` for its
options (rate, gaps, connect bursts).
