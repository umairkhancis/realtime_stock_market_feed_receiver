# Clean architecture, in Rust — the receiver

This document records how the receiver was reorganised and, for each decision,
the Rust guidance behind it. It changes no behaviour: `help`, `summary`,
`verify`, `one` and the unknown-command error print byte-for-byte what they
printed before, `listen` differs only in its wall-clock timing measurements,
and the `received.csv` it writes is byte-identical to the transmitter's
`feed.csv` on a lossless capture, as it was before.

It is the companion to `docs/clean_arch.md` in the transmitter repo. Where a
decision is the same, this document says so and does not re-argue it. Where the
receiver diverges — and there are four places it does — the divergence is the
interesting part, so those get the space.

## 1. The rule

Clean Architecture is one rule and a lot of consequences. The rule is that
**source dependencies point inward**. An inner ring may not name an outer one.
Everything else — ports, adapters, presenters — exists to make that rule
survivable when the inner ring nevertheless needs something from outside.

Four rings:

```text
              ┌──────────────────────────────────────────┐
              │            presentation                  │   cli, console,
              │  ┌────────────────────────────────────┐  │   report, summary,
              │  │          application               │  │   format, banner
              │  │  ┌──────────────────────────────┐  │  │
   infra──────┼──┼─▶│           domain             │◀─┼──┼── ports, use cases,
   csv, udp   │  │  │ message, codec, symbols, loss│  │  │   capture, reports
              │  │  └──────────────────────────────┘  │  │
              │  └────────────────────────────────────┘  │
              └──────────────────────────────────────────┘
```

`infrastructure` and `presentation` are siblings in the outer ring; neither may
name the other, and both may name the two inner ones.

## 2. What was actually wrong

The old layout was eight sibling modules under `src/`, plus a `lib.rs` that was
also the program. Flat is not a sin — the
[Rust Book, ch. 7](https://doc.rust-lang.org/book/ch07-00-managing-growing-projects-with-packages-crates-and-modules.html)
is clear that you split modules when a file stops being coherent, not on a
schedule. But five specific arrows had gone the wrong way, and a flat list is
exactly the shape that hides them:

| Violation | Where | Why it matters |
|---|---|---|
| `codec.rs` tests called `crate::hex` / `crate::format_price` | domain → presentation | a wire-format rule asserting how a price *looks* |
| `receive.rs` held four `println!`, two of them in the capture loop | infrastructure → terminal | see below; this one is not cosmetic |
| `detect.rs` and `compare.rs` each ended in a `print_*` function | domain → terminal | loss arithmetic that knows about stdout |
| `Detection::blind_spot_note()` returned a formatted English sentence | domain → presentation | a rendering concern wearing a domain type's clothes |
| `lib.rs` held four commands *and* `File`/`UdpSocket`/`BufWriter` plumbing | application ≡ infrastructure | `listen()` could not run without a socket and a filesystem |

Plus two smaller ones: `feed.rs` (the CSV format) owned `SymbolMap`, which is a
rule about *which ITCH messages carry a ticker* and not about CSV at all; and
`main.rs` mixed argv handling with a 30-line usage constant and a dispatch
table, which is the thing a binary's `main` can least afford since it cannot be
integration-tested.

**The `println!` in the capture loop deserves its own paragraph**, because on
this side of the pipeline it is the difference between measuring the network
and measuring yourself. `receive.rs` said so in its own module doc: 100,000
`println!` to a terminal at a 100 µs budget each makes the terminal emulator
the bottleneck, the socket receive buffer overflows, and the loss looks like
the network's fault. The old code handled this with discipline — the progress
line fired on a timer — but discipline is what a comment buys you. A port is
what makes it structural, and `tests/architecture.rs` now forbids `println!`
anywhere in `infrastructure` for exactly this reason. That rule is stricter
than the transmitter's, and it is stricter because the receiver's failure mode
is worse: a transmitter that prints too much sends late, and says so; a
receiver that prints too much drops datagrams and blames the wire.

## 3. Where everything went, and why

### `domain` — what an ITCH feed is, and what a gap in one is

`message`, `codec`, `symbols`, `loss/{sequence, detect, compare}`. The test
this ring is held to: *would this code change if we swapped UDP for TCP, or CSV
for Parquet?* If yes, it is not domain.

**`codec` and `message` are byte-identical to the transmitter's copies**, and
the placement argument is the transmitter's, unchanged: ITCH's byte layout *is*
the product. `tests/architecture.rs` has a fourth test here that the
transmitter does not need — `the_codec_and_the_message_records_name_nothing_outside_the_domain`
— because these two files are shared by copy between two repositories, and the
moment either side's copy grows a `use crate::infrastructure::…` the copies
stop being interchangeable and the golden vector stops being a contract.

**Loss detection is domain, and this is the placement worth arguing.** The
instinct is that "detect loss in a stream" is something the *program does*, and
therefore an application concern. Run the test instead. Would `detect.rs`
change if the datagrams arrived over TCP? No. If the ground truth were Parquet?
No. Would it change if NASDAQ added a message type that carried an order
reference? Yes — immediately and substantially, because the whole table in its
module doc (which messages are provable, which are invisible) is derived from
what each ITCH message *carries*. `D` and `X` are undetectable losses because
of the ITCH spec, not because of anything this program chose. That is the
signature of an enterprise rule.

`loss` splits three ways. `sequence` is the arithmetic — what a monotonic
counter says about what is missing from it — and it came out of `detect`
because both of that module's estimators are the same arithmetic over different
columns, and testing it once directly is sharper than testing it twice through
a book replay. `detect` is the ITCH-specific part: the book replay and the
per-locate bookkeeping. `compare` is the answer-key diff. A 560-line module
where a caller who wants to know what a stride is has to pull in a `HashMap`
book replay is a cohesion problem, not a line-count problem — the same argument
the transmitter used to split `market`.

**`SymbolMap` moved from `feed.rs` to `domain::symbols`.** Only 'A' and 'F'
carry an ASCII ticker; every other message identifies its instrument by locate
alone, so a receiver either reads the map out of band or learns it from the add
stream. *That* is a fact about ITCH. Parsing the transmitter's
`feed.symbols.csv` is a fact about CSV, and it stayed in
`infrastructure::csv::serde`. The old code had both in one file, which is how
`SymbolMap::learn` — a pure statement about the wire format — ended up in the
module that owns a header row.

**`fixtures` is `#[cfg(test)]` and stays that way.** Its own module doc gives
the reason: the receiver has no market generator and should not grow one, since
a receiver that can only be tested against its own idea of a feed is testing
nothing. That has a consequence for test placement, in §6.

### `application` — what the program does

Use cases over ports: `receive::capture_feed`, `verify::{verify_against,
verify_stored, dump_capture}`, `slice_one::receive_single`, plus `capture`
(the arena) and the `ports` themselves.

`capture_feed` is the clearest before/after. It used to be the first third of
`lib.rs::listen()` and it opened a socket. It now reads:

```rust
pub fn capture_feed(
    source: &mut impl DatagramSource,
    config: &ReceiveConfig,
    observer: &mut dyn CaptureObserver,
) -> Result<CaptureOutcome> {
    let (capture, report) = source.capture(config, observer)?;
    let (messages, failures) = capture.decode_all();
    let detection = Detection::run(&messages);
    Ok(CaptureOutcome { report, messages, failures, detection })
}
```

No `UdpSocket`, no `println!`. That is dependency inversion doing its job: the
use case names `DatagramSource`, never `UdpDatagramSource`.

**`Capture` is the mirror of the transmitter's `EncodedFeed`** — one contiguous
arena plus an `(offset, len)` index, so nothing variable happens per datagram —
and it moved inward for the same reason `PaceStats` did: it is what a run
*produces*, not how a socket works. Its three fields went private and it gained
`push()`, `reserve()` and `payload_bytes()`, so the adapter folds a datagram in
rather than poking three vectors across a ring boundary
([C-STRUCT-PRIVATE](https://rust-lang.github.io/api-guidelines/predictability.html#c-struct-private)).
The three lengths are now in step by construction rather than by care.

**`ReceiveReport::first_peer` changed type, from `SocketAddr` to `String`.**
That single field was the last `std::net` in the application ring, and the
architecture test caught it. The adapter renders it — the same trick the
transmitter's `TransmitStart::local` uses. Note *where* it renders it: the
comparison that counts distinct peers still runs on a `SocketAddr` local to the
loop, and the `to_string()` happens once after the stream ends. Rendering per
datagram would have put an allocation inside the budget, which would have been
a real cost paid for a layering win — the wrong trade, and avoidable.

**There is no `listen` use case, on purpose.** `listen` is a sequence of port
calls and domain results with rendering between them: capture, render; detect,
render; verify, render; dump, render. Wrapping that in an application function
would require an observer with six methods whose only job is to re-emit what
the CLI already prints — the kind of thing that earns clean architecture its
reputation for cardboard layers. The composition root calls the three use cases
in order, which is what a composition root is for. This is the same judgement
the transmitter's document records for its `summarise`; knowing where *not* to
put a seam is as much a part of this as knowing where to put one.

### `infrastructure` — the adapters

`csv` (format + `CsvFeedStore` + `CsvSymbolSource`), `net` (bind policy + the
two UDP adapters). Every one implements a port declared one ring in.

`csv` is split so the *format* can be tested without a disk: `serde` reads and
writes over any `BufRead`/`Write` and round-trips through a `Vec<u8>`; `store`
is the thin part that knows about `File` and `PathBuf`. That split follows the
API guidelines'
[C-GENERIC](https://rust-lang.github.io/api-guidelines/flexibility.html) advice
to accept `impl Write` rather than a concrete `File`, and the payoff is that
every one of the CSV format tests — including the central claim that a lossless
capture dumps a byte-identical file — never creates one.

`net/udp.rs` holds both sockets, and is the only file in the crate that names
`UdpSocket`. `BIND_ADDRESS` is a constant there rather than a literal at two
bind sites, because `0.0.0.0` versus `127.0.0.1` is the single most expensive
mistake available on this side of the pipeline: binding loopback works
perfectly on one machine and silently receives nothing from another.

### `presentation` — the terminal, and the composition root

`cli` (usage, dispatch, operator defaults), `console` (the observer and the
transport report), `report` (the two loss reports), `summary`, `format`,
`banner`.

**`banner` earns its own file for one reason:** `figlet-rs` and `colored` are
the crate's only third-party dependencies, and that is the only file that names
them. Clean Architecture says frameworks belong in the outermost ring where
they can be deleted without consequence; here that is literally true, and
`tests/architecture.rs` asserts it. Given a brief that says *"implementation
should be non-standard crates dependency free"*, having the two crates that
violate it quarantined in one deletable file is not a stylistic win.

**`report::blind_spot_note` is the move worth pointing at.** It was
`Detection::blind_spot_note(&self) -> String` — a method on a domain type that
returned a formatted English sentence with a percentage in it. It reads like
domain because it is *about* the domain, but "of 2996 received messages
(27.9%)" is rendering, and a domain type that can produce it is a domain type
that has an audience. The domain now supplies `unverifiable`, `total` and
`blind_fraction()`; the sentence is written one ring out. This is the same
correction as `format_price`, one level less obvious.

## 4. The ports, and how they dispatch

Five traits in `application/ports.rs`. The two Rust-specific decisions are the
transmitter's, and hold here for the same reasons:

**Associated `Error` types, not `Box<dyn Error>`.** Each port carries
`type Error: std::error::Error + 'static`, following `FromStr`, `TryFrom` and
`Iterator`. The adapter keeps its own concrete error — `FeedError`,
`io::Error` — and the `'static + Error` bound is exactly what makes
`Box<dyn Error>: From<E>` apply, so a use case still writes `?`. Boxing at the
trait would have thrown away the adapter's error type at the boundary and
forced every implementor to allocate. The payoff shows up in the tests: the
`MemoryStore` in `verify.rs` uses `Infallible` as its error, which is only
expressible because the port did not pre-decide the type.

**Generics for collaborators, `dyn` for the presenter.**

```rust
pub fn capture_feed(
    source: &mut impl DatagramSource,
    config: &ReceiveConfig,
    observer: &mut dyn CaptureObserver,
) -> Result<CaptureOutcome>
```

`DatagramSource` is chosen once at the composition root, so `impl Trait`
monomorphises and the abstraction costs literally nothing at run time — the
standard answer to "won't the indirection be slow?", and it matters more here
than on the transmitter, since this is the one program in the pair with a hard
real-time budget it does not control. `CaptureObserver` is `&mut dyn` because
it is passed *through* the source into the receive loop; making it generic
would monomorphise the transport over the presenter for no benefit, and it
fires once per second, so one vtable hop is beneath measurement. Static
dispatch where it is hot and chosen at compile time; dynamic dispatch where it
is cold and passed along.

`CaptureObserver` is the *output port* — a presenter, in Clean Architecture's
vocabulary. Every method has a default no-op body, so `SilentObserver` is three
tokens and a test observer is an `impl` block with the one method it cares
about.

### The two boundaries drawn pragmatically

**`DatagramSource::capture` takes a whole run, not one datagram.** The more
textbook split puts the receive loop in the use case behind a per-datagram
port. It was not taken, and the reason is sharper than the transmitter's
version of the same trade: the loop takes exactly one `Instant::now()` and it
has to be the instant `recv_from` returns, not one trait call later, because
that timestamp *is* the inter-arrival measurement the whole `arrival timing`
report is built from. A per-datagram port would insert a vtable dispatch
between the kernel and the clock, and would need a `Clock` port beside it. The
property that actually matters — a use case that never names `std::net` — is
preserved either way.

**`SymbolSource` is a separate port from `FeedStore`.** The transmitter bundles
the two, deriving `feed.symbols.csv` from `feed.csv` inside one store. The
receiver cannot: `summary` reads its messages from the dump *this* program
wrote and its names from the map the *transmitter* wrote, and a bundled store
would have to invent a `received.symbols.csv` that nothing produces. The
derivation still exists — `CsvSymbolSource::beside()` — and a CLI test asserts
it agrees with the transmitter's convention.

## 5. Rust-specific choices

**No `mod.rs`.** `src/domain.rs` + `src/domain/`, not `src/domain/mod.rs`. The
2018-and-later path style, and the one clippy's
[`self_named_module_files`](https://rust-lang.github.io/rust-clippy/master/#self_named_module_files)
lint prefers. Practically: ten files named `mod.rs` in your editor's tab bar is
its own argument.

**`application::Result<T = ()>`, not `Fallible`.** The old alias fixed `T` at
`()`, which is not a shape any std alias takes — and the receiver actually
needed the parameter, since `capture_feed` returns a `CaptureOutcome`. The
replacement is module-qualified — `io::Result`, `fmt::Result`,
`application::Result` — per the
[API guidelines' naming conventions](https://rust-lang.github.io/api-guidelines/naming.html),
with a default type parameter so the common spelling stays short.

**Boxed errors in the application, concrete enums in the layers.** The
[Rust Book's I/O project](https://doc.rust-lang.org/book/ch12-03-improving-error-handling-and-modularity.html)
draws exactly this line, and it is kept: `CodecError` and `FeedError` stay
concrete enums with hand-written `Display`/`Error` impls (no `thiserror` — the
brief forbids the dependency), because a caller might match on them. The
application layer boxes, because its only consumer is `main`, which prints and
exits.

**A thin `main.rs`.** Twenty-two lines: banner, argv, exit code. Everything
else is in the library crate beside it, which is the split the Rust Book
recommends in the same chapter, for the same reason — a binary's `main` cannot
be integration-tested, so as little as possible should live there. `cli::run`
takes `&[String]` rather than reading the environment itself, which is what
makes the three CLI tests possible at all.

## 6. Enforcing it

**Rust modules do not enforce acyclicity.** `crate::domain` can name
`crate::presentation` and the compiler will agree. So directory layout alone is
a convention that documents intent and does nothing to preserve it — six months
of commits and it is a lie. There are exactly two ways to make the rule real:

1. **A workspace.** One crate per layer. Cargo *does* refuse a dependency cycle
   between crates, so the rule becomes a compile error. This is the right
   answer for a large codebase and the wrong one at 5,500 lines: you pay for it
   in build graph complexity, four `Cargo.toml`s, and `cargo test` no longer
   meaning "test everything".
2. **A test.** `tests/architecture.rs` reads every file under `src/`, strips
   comment lines (so a doc link pointing outward is prose, not a dependency),
   and asserts four things: no inner ring names an outer one; `colored` and
   `figlet_rs` are reachable from `presentation/banner.rs` and nowhere else;
   the only top-level modules are the four rings, so a file dropped into `src/`
   cannot escape governance by belonging to no layer; and the two files shared
   by copy with the transmitter depend on nothing but `crate::domain`.

The second was chosen. It is ~230 lines, it runs in microseconds, and its
failure message names the file, the forbidden symbol, and the reason — which is
how the `SocketAddr` in `ReceiveReport` was found:

```text
the dependency rule is broken in 1 place(s):
  application/receive.rs: names `std::net` — a use case that opens a socket has swallowed its adapter
```

The transmitter's copy of this test says to migrate to a workspace *when a
second binary appears*, and names this receiver as the candidate. That is now
worth restating precisely, because the two repos are separate and the shared
files are shared by copy: the workspace becomes worth building when `domain/
message.rs` and `domain/codec.rs` become a `itch-wire` crate that both binaries
depend on. Everything above is arranged so that is a move, not a rewrite —
neither file names anything outside `crate::domain`, and a test enforces it.

### Where the cross-layer tests are, and why they are not in `tests/`

The transmitter promotes properties that span two rings into
`tests/feed_round_trip.rs`. The receiver keeps its two — a lossless capture
dumps a byte-identical CSV, a lossy one does not — as unit tests in
`infrastructure/csv/serde.rs`, and the reason is `fixtures`. Building a known
feed needs `domain::fixtures`, which is `#[cfg(test)]` on purpose; an
integration test sees only the public API, so promoting these two would mean
publishing a feed generator from a receiver whose stated design position is
that it must not have one. The layering cost is small and named here; the
alternative cost is a public API that invites exactly the mistake the module
doc warns about.

## 7. What changed, precisely

Behaviour: nothing. Verified by building the pre-refactor binary from `HEAD` in
a worktree and diffing stdout, stderr and exit status for every command:
`help`, `-h`, `--help`, no arguments, an unknown command, `summary` and
`verify` are byte-identical; `one` is byte-identical modulo the sender's
ephemeral port; `listen` is byte-identical modulo the `source` line and the
`arrival timing` block, which are wall-clock measurements that differ between
any two runs of the same binary. `listen` was exercised both ways — with
injected loss, so the detection, comparison and failure paths ran, and with a
complete stream, so the success path ran and wrote a `received.csv` that is
byte-identical between the two binaries *and* to the transmitter's `feed.csv`.

Signatures that changed, all of them to fix a dependency arrow:

- `listen()` / `summarise()` / `verify()` / `listen_one()` no longer exist as
  no-argument crate-root functions. They are use cases taking ports
  (`capture_feed`, `verify_against`, `verify_stored`, `dump_capture`,
  `receive_single`) plus CLI handlers that wire the adapters.
- `receive(&ReceiveConfig)` → `DatagramSource::capture(&ReceiveConfig, &mut dyn
  CaptureObserver)`. The four `println!` it contained became two observer
  methods.
- `ReceiveReport::first_peer` is a `String`, rendered by the adapter, not a
  `SocketAddr`.
- `Capture`'s fields went private; the adapter uses `push`, `reserve` and
  `payload_bytes`.
- `Detection::blind_spot_note()` → `presentation::report::blind_spot_note(&d)`.
- `SymbolMap` moved from `feed` to `domain::symbols`; `read_symbol_table` stayed
  in the CSV adapter and now returns the `BTreeMap` the map is built from.
- `print_report`, `print_detection`, `print_comparison` moved to
  `presentation::{console, report}`; the decode-failure block that was inline in
  `listen()` became `console::print_decode_failures`.
- `Fallible` → `application::Result<T = ()>`.
- `DEFAULT_PATH` / `TRUTH_PATH` / `DUMP_PATH` → `cli::{DEFAULT_SYMBOLS_CSV,
  DEFAULT_TRUTH_CSV, DEFAULT_DUMP_CSV}`; `DEFAULT_PORT` moved to
  `infrastructure::net`, where a bind address belongs.

Tests: 81 passing, up from 53. One dead test deleted
(`a_bare_port_still_means_slice_one`, which asserted only that `"9000".parse::
<u16>()` succeeds and `"listen".parse::<u16>()` does not — it documented an
argument shape the CLI has never implemented). The rest moved with their code,
and the additions are all at seams the refactor created: the ports get fake
implementations (`MemoryStore`, `Canned`) that prove the use cases run with no
filesystem and no socket; the capture loop gets a recording observer that
proves it narrates itself through the port and not by any other route; the
adapters get tests for the paths and locations they report; `format` gets the
price rendering the codec used to assert; and `tests/architecture.rs` adds four.

## 8. What was deliberately not done

- **`presentation::summary` still interleaves computation with `println!`.**
  Its pure half (`focus_mids`, `realized_vol`, `choose_focus`) is already
  separable and already tested. Splitting it into a `domain::analytics` that
  returns a struct and a presenter that renders it is the honest next step; it
  was skipped because it is a real implementation change, and this pass was a
  structural one. The transmitter's document records the identical hold on its
  own `summary`, and the two should move together when either does.
- **`cli::verify` still returns `Err("verification failed")` unconditionally**,
  even when the comparison is perfect. That is a bug, it predates this work, and
  fixing it would change what the command does — which is out of scope for a
  refactor that promised not to. It is called out in the code as well as here so
  it is not mistaken for intent.
- **`cli::load_symbols` still panics** rather than erroring when the symbol map
  is missing, and still does not fall back to `SymbolMap::learn`, which exists
  and would work. Same reason; same treatment.
- **The usage text still advertises flags nothing parses** (`--port`, `--expect`,
  `--csv`, `--dump`, …) and an exit-status contract (`2 = error`) that `main`
  does not implement. Argument parsing is a feature, not a layering fix.
- **Six clippy warnings survive** (three byte-string literals, two
  `useless_format`, one `needless_return`) — all of them in code that moved
  verbatim, and all of them consequences of the three items above. One more,
  a collapsible `if` in `csv/store.rs`, is inherited: that adapter is a
  deliberate line-for-line port of the transmitter's, and keeping the two
  readable side by side is worth more than the lint.

## References

- [The Rust Programming Language, ch. 7 — Managing Growing Projects](https://doc.rust-lang.org/book/ch07-00-managing-growing-projects-with-packages-crates-and-modules.html)
- [The Rust Programming Language, ch. 12 — An I/O Project](https://doc.rust-lang.org/book/ch12-03-improving-error-handling-and-modularity.html) — the `main.rs` / `lib.rs` split, and boxed errors at the binary boundary
- [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/) — C-GENERIC (accept `impl Write`), C-STRUCT-PRIVATE, C-COMMON-TRAITS, naming
- [Rust Error Handling — std, and the case for concrete library errors](https://doc.rust-lang.org/std/error/trait.Error.html)
- [clippy: `self_named_module_files`](https://rust-lang.github.io/rust-clippy/master/#self_named_module_files) — the no-`mod.rs` layout
- `docs/clean_arch.md` in the transmitter repo — the other half of this argument
- Robert C. Martin, *Clean Architecture* — the dependency rule, ports and adapters, presenters

---

## The whole thing, traced

```text
main.rs                                  cli::run(["listen"])
└─ presentation::cli::listen()           builds UdpDatagramSource, ConsoleObserver,
   │                                     CsvFeedStore ×2, ReceiveConfig
   ├─ application::capture_feed(source, cfg, observer)
   │  ├─ source.capture()      ──port──▶ UdpDatagramSource
   │  │                                  ├─ bind(BIND_ADDRESS, port)
   │  │                                  ├─ observer.on_listening() ──port──▶ ConsoleObserver
   │  │                                  └─ loop: recv_from / Instant::now()
   │  │                                           capture.push(&buf[..n], t)
   │  │                                           observer.on_progress()
   │  ├─ capture.decode_all()  ─────────────────▶ domain::codec::decode
   │  └─ Detection::run()      ─────────────────▶ domain::loss::{sequence, detect}
   ├─ presentation::console::print_report(&outcome.report)
   ├─ presentation::console::print_decode_failures(&outcome.failures, …)
   ├─ presentation::report::print_detection(&outcome.detection)
   ├─ application::verify_against(truth, &outcome.messages)
   │  ├─ truth.load()          ──port──▶ CsvFeedStore ─▶ csv::serde::read_feed
   │  └─ compare()             ─────────────────▶ domain::loss::compare
   ├─ presentation::report::print_comparison(&verification.comparison)
   └─ application::dump_capture(dump, &outcome.messages)
      └─ dump.save()           ──port──▶ CsvFeedStore ─▶ csv::serde::write_feed
```

Every arrow crossing a ring boundary inward is a direct call. Every arrow
crossing outward goes through a port. `cli::listen` is the only function that
sees all four collaborators, and it names none of them to the layers below it.
