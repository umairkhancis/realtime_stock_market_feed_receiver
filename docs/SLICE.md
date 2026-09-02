## Slice 1

A small test where transmitter emits an ITCH message as a UDP packet and receiver on other machine with rust dependency free implementation and recover the same data.

## Slice 2

Make changes to implement the corresponding receiver program as per this transmitter and its command.

Now I'll build it. Let me first check how far the two codebases have already diverged:

  Ran 3 shell commands

The wire contract is now shared byte-for-byte. Now the receiver-specific modules — starting with a deterministic test fixture, since the detectors are the part that must be tested against known loss:

  Ran 11 shell commands

Three failures, all real. Let me confirm the cause of the off-by-one before touching the tests:

  Ran 7 shell commands

Confirmed: locate 2's last add was dropped, so no later value exists to reveal the gap. A real boundary limitation, not a counting bug. Let me fix all three properly:

  Ran 1 shell command

All 57 pass. Now the real end-to-end run — transmitter to receiver, 100,000 datagrams at 10k/s:

  Ran 2 shell commands

0% loss end to end. Now verifying the two claims the output makes — byte-identical dump, and that loss is actually caught:

  Ran 2 shell commands

Exact cross-validation. The truth was 367 adds + 111 executions + 1,187 D/X; the detector found 367 and 111 exactly, and the 1,187 it missed is precisely the blind spot it declares. Final checks:

  Ran 1 shell command

Done. Both sides run green.

The verification run

Receiver first, then tx send --csv data/feed.csv --dest 127.0.0.1:9000 --rate 10000:

┌───────────────┬──────────────────────────────────────────────────────────┐
│               │                                                          │
├───────────────┼──────────────────────────────────────────────────────────┤
│ datagrams     │ 100,000 sent / 100,000 received                          │
├───────────────┼──────────────────────────────────────────────────────────┤
│ payload       │ 2,948,454 bytes both sides                               │
├───────────────┼──────────────────────────────────────────────────────────┤
│ framing       │ 0 length mismatches, 0 unknown type bytes                │
├───────────────┼──────────────────────────────────────────────────────────┤
│ inter-arrival │ p50 100.0 µs, mean 100.0 µs, p99 110.6 µs, max 1123 µs   │
├───────────────┼──────────────────────────────────────────────────────────┤
│ ground truth  │ 0% LOSS — every message arrived, in order, byte for byte │
├───────────────┼──────────────────────────────────────────────────────────┤
│ dumped CSV    │ cmp against feed.csv: identical, 4,912,509 bytes         │
└───────────────┴──────────────────────────────────────────────────────────┘

Then I re-ran it with a deliberately lossy feed (1,665 rows removed, weighted toward D/X, plus one 250-message burst):

- truth: 367 adds + 111 executions + 1,187 D/X
- content detectors found: 367 adds, 111 executions — exact, no estimate
- the 1,187 they missed is precisely the blind spot they declare
- ground truth found all 1,665 and identified the burst as "largest 250 consecutive"
- exit code 1

That's the two-tier design working: inference is exact where it can see, honest where it can't, and the CSV covers the rest.

What's in it

New: receive.rs (capture loop), detect.rs (content-based loss detection), compare.rs (ground-truth diff), feed.rs (CSV + SymbolMap), summary.rs (the three tables), fixtures.rs (test-only). Rewritten: lib.rs. codec.rs and model.rs are now byte-identical to the transmitter's — the golden vector is the wire contract and it means nothing if each side keeps its own offsets.

rx listen --expect 100000 --csv feed.csv --dump received.csv --focus NVDA
rx verify --csv feed.csv --received received.csv     # offline, no network
rx summary --csv received.csv
rx one 9000                                          # slice 1, unchanged

Three decisions worth flagging:

The stride is inferred, not hardcoded. The transmitter strides order references by 8 because it runs 8 symbols. A receiver that assumed 8 would report phantom loss on every add the day a ninth symbol is listed. So the receiver derives it from the smallest observed delta — deliberately not the GCD, which divides every delta by construction and so can never report an inconsistency. The test fixture uses 4 symbols precisely so a hardcoded 8 would fail loudly.

A failing test found a real limitation. Gap arithmetic needs a value on both sides of the gap, so an add lost before a symbol's first survivor or after its last leaves no trace. I'd written the test asserting exact recall; it came back 868 vs 869. The cause was a genuine boundary blind spot, not a counting bug, so the test now asserts the strongest true statement instead, and the limitation is stated in the printed report. Same root cause as tail loss being invisible.

The hot loop does four things: recv_from, one Instant::now(), a length check, and a copy into a preallocated arena. No decoding, no per-message allocation, no printing — all analysis runs after the stream ends. std::net::UdpSocket exposes no SO_RCVBUF setter (that needs libc), so the only defence against the ~787 KB default buffer is not falling behind.

main.rs now exits 0 verified / 1 messages missing / 2 receiver error, so this is scriptable. Nothing committed in either repo. The receiver still carries figlet-rs and colored for the banner — same pre-existing tension with the brief as the transmitter.

✻ Brewed for 15m 19s · done 1:22 PM