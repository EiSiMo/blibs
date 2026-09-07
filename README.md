# blibs

Search the libraries of Berlin and Brandenburg from the command line. `blibs` answers one
question:

> Is this book in one of my libraries, is it in right now, and where is it on the shelf?

It is a client for the **KOBV** union catalogue (`k2`, ~88 institutions — university
libraries, research institutes, archives, museums, and the public library networks of
Berlin and Brandenburg) and for `voebb.de`, the only place that knows which *branch* of
the Berlin public library network (VÖBB) holds a given copy.

## What this does not do

- **No online article index.** KOBV federates catalogue records, not article databases.
- **No nationwide search.** Scope is Berlin/Brandenburg; that is the whole point of a
  regional tool.
- **No return dates or holds.** Those live behind a library account, and `blibs` signs in
  nowhere — a copy that is out is reported as `on loan`, never with a guessed due date.
- **No holdings runs for journals.** The availability service gives one status for the
  *title*, with no volume — a green light next to a journal would otherwise read as a
  claim about the specific year you want.

## Installation

```sh
cargo install --path .
# or, for a local build:
cargo build --release
```

Requires Rust 1.85 or later (edition 2024).

## Quickstart

```sh
blibs search Kafka Prozess
blibs search "Der Vorleser" --at HU,STABI,AGB
blibs search --author Kafka --year 1953 --format book
blibs search "Der Vorleser" --at AGB --available
blibs show almafu_BV008885798 --at STABI,HU
blibs libraries --find grimm
blibs libraries --near 52.52,13.39
```

There is no config file. "My libraries" is a shell alias:

```sh
alias bl='blibs search --at STABI,HU,AGB'
```

Quoting is the whole trick: an argument in shell quotes is searched as a phrase, one
without is a word, and several words must all occur.

## Output

### With `--at`: grouped by location

```
$ blibs search "Der Vorleser" --at HU,STABI,AGB

HU Berlin · 6 results · showing 2

  ●  Der Vorleser : Roman                Schlink, Bernhard   1997   almahu_BV011234567
       Grimm-Zentrum, 7. OG               GM 5000 S345 V9      available
       ZwB Germanistik, UG                GM 5000 S345 V9      on loan
  ◐  Bernhard Schlink, Der Vorleser      Mittelberg, E.      2004   almahu_BV019876543
       Grimm-Zentrum, 5. OG               GM 5000 S345 V9 M6   reference only

Stabi Berlin · 3 results · showing 1

  ●  Der Vorleser : Roman                Schlink, Bernhard   1995   almafu_BV010111222
       Haus Potsdamer Straße              1 A 234 567          available

AGB (VÖBB) · 35 results · showing 2

  ●  Der Vorleser                        Schlink, Bernhard   1997   voebb_SAK13776205
       Belletristik                       Schlink              available
  ○  Der Vorleser                        Schlink, Bernhard   2012   voebb_SAK14200311
       Belletristik                       Schlink              on loan

●  available      ◐  reference only      ○  on loan
```

One block per location, in the order given to `--at`. Each block's heading carries that
location's true hit count (`HU Berlin · 6 results`), not the count of the combined search;
`showing N` appears whenever the block prints fewer than that. A record can appear in more
than one block if more than one of your locations holds it — that is intentional, not a
duplicate. There is no flag to turn the item lines off; use `--limit` for less.

### Without `--at`: one flat line per hit

```
$ blibs search Kafka Prozess

774 results for "Kafka Prozess" · showing 1-3

  1 ●  Der Prozess                             Kafka, Franz     1953  almafu_BV008885798
  2 ◐  Der Process : Roman                     Kafka, Franz     1990  b3kat_BV005550341
  3 ○  Der Prozeß : Roman                      Kafka, Franz     2008  almafu_BV035123456

●  available somewhere      ◐  reference only      ○  currently unavailable
```

No location to group by, so no item lines here either — run `show` on a record id for
those. The legend only lists symbols that actually occur in the output above it.

### `show`: one title in full

```
$ blibs show almafu_BV008885798 --at HU,FU,ZLB

Der Prozess
Roman

  Kafka, Franz (1883-1924)          author        GND 118559230
  Brod, Max (1884-1968)             editor        GND 118515012

  Published    S. Fischer, Frankfurt am Main, 1953
  Edition      3. Auflage
  Extent       345 Seiten
  Language     German
  Format       Book
  ISBN         9783596294331
  Subjects     Deutsche Literatur · Roman · Prag
  Online       https://d-nb.info/… (table of contents)

Holdings

  ● HU Berlin — Humboldt-Universität zu Berlin, Universitätsbibliothek · 2 of 2 available
      ZB Grimm-Zentrum, 7. OG / Bereich B      96 A 10064        available
      ZwB Germanistik/Skandinavistik, UG       GM 4004 K64       available
  ◐ FU Berlin — Freie Universität Berlin, Universitätsbibliothek
      Philologische Bibliothek, Ebene 1        GM 4004 K64 A9    reference only
  ○ ZLB — Zentral- und Landesbibliothek Berlin
      Amerika-Gedenkbibliothek                 Kaf 3             on loan

  a copy on loan carries no due date here — return dates and holds are only in the library's own catalogue, behind a patron login

  also at: Stabi Berlin · TU Berlin · EUV Frankfurt (Oder) · UP Potsdam

almafu_BV008885798
```

Libraries named in `--at` are listed first, in that order; everything else is one
`also at:` line. Without `--at`, every holding is simply listed and nothing is marked.

## For agents

`--json` prints one JSON document on stdout and nothing else; errors go to stderr, also as
JSON. A shortened `search` document:

```json
{
  "total": 774,
  "shown": 2,
  "window": { "fetched": 50, "after_filter": 2 },
  "engines": ["kobv", "voebb"],
  "at": [
    { "key": "HU", "isil": "DE-11", "branch": null, "engine": "kobv", "total": 6 }
  ],
  "availability": "fetched",
  "notes": [],
  "records": [
    {
      "id": "almafu_BV008885798",
      "engine": "kobv",
      "title": "Der Prozess",
      "format": "book",
      "online": false,
      "holdings": [
        {
          "isil": "DE-11",
          "alias": "HU",
          "mine": true,
          "summary": "available",
          "items": [
            { "location": "ZB Grimm-Zentrum, 7. OG / Bereich B",
              "call_number": "96 A 10064", "status": "available" }
          ]
        }
      ]
    }
  ]
}
```

**Stability rule:** fields may be added over time; renaming or removing one is a breaking
change.

- `window` is `{ fetched, after_filter, filtered }` — how many records the query actually
  returned, how many survived a client-side filter such as `--format`, and whether such a
  filter was set at all. Without the first two, "nothing found" and "nothing in this
  window" both look like an empty result; `filtered` is always present because
  `after_filter == fetched` is true both when no filter ran and when one matched
  everything.
- **With `--format` or `--language`, the window is the whole addressable set.** It is one
  anchored block of 50 records and `--page` walks the *matches inside it*, so pages
  partition those matches and no page reaches past the block. Without a filter `--page`
  steps the result itself, as `(page - 1) * limit + 1`.
- `window.before_available` is there only when `--available` ran, and says how many of the
  displayed records it judged. The survivors are `shown`, so the hidden ones are the
  difference; its absence is how a page that was always this short is told apart from one
  the filter thinned out. `at[].total` is untouched by the filter — it stays that
  location's true hit count, and only `at[].records` is filtered.
- `availability` is `"fetched"` or `"skipped"` — whether the status service was asked at
  all, so an empty `items` list is never ambiguous.
- `notes` carries footnotes such as a skipped diagnostic record; it is `[]` when there is
  nothing to say.
- `at[]` gives, per `--at` location, the resolved ISIL/branch, which engine answered, and
  that location's true hit count — an agent reconstructs the grouped view from this and
  `holdings[].isil` / `items[].branch` itself; `records` stays record-centric, with each
  record listed exactly once even if it would appear in several blocks.

### Exit codes

| Code | Meaning |
| --- | --- |
| 0 | found at least one hit |
| 1 | searched and found nothing; also an unknown record id or no library matched |
| 2 | usage error — nothing was sent |
| 3 | network error — DNS, TLS, connection refused, timeout |
| 4 | service unavailable — HTTP 429/503, backoff exhausted |
| 5 | the catalogue rejected the query |
| 6 | unexpected response — HTTP 200 whose content failed the plausibility check |

Error object shape:

```json
{
  "error": {
    "code": 2,
    "kind": "unknown_library",
    "message": "unknown library \"STABI2\"",
    "hint": "run `blibs libraries --find <name>` to look up a library"
  }
}
```

## Good to know

- **`--author` is always searched as a word list, never a phrase.** The author index holds
  authority forms; `"Franz Kafka"` as a phrase finds almost nothing, so a natural name is
  split into words instead.
- **`--year` means "was running in this year", not "was published in this year".** For a
  journal that ran 1923–1991, `--year 1960` matches and `--year 1922` does not.
- **`--sort`, `--format` and `--language` only see the fetched window**, not the whole
  result — the catalogue itself cannot sort or filter by material type or language. A
  short result after `--format` is stated as a window effect, never as "none exist". With
  `--format` or `--language` that window is also all `--page` can reach: it walks the
  matches inside one anchored block of 50 records rather than stepping the result.
- **The catalogue does not order a result stably.** Two identical requests return
  different records in a different order — measured against `sru.kobv.de/k2` directly. So
  consecutive pages can overlap or leave records out, and from page two on a
  `result_order_unstable` note says so. The cache hides this: repeated identical calls
  look stable until `--no-cache`.
- **`--available` thins the page out, it does not reload.** The status is only asked about
  for the records that are shown, so ten hits can come back as four — use a larger
  `--limit` to see more. Reference stock does not count as available, and a record the
  catalogue states no status for drops out too; the footer says how many went and how many
  of those were never answered. With several `--at` locations it acts per block: a record
  can survive under one heading and vanish from another. Combining it with
  `--no-availability` is a usage error — there would be nothing to filter on.
- **Subject headings are mixed-language and untranslated.** `--subject Recht` and
  `--subject law` are different searches; both can return hits.
- **ISBN check digits are verified before anything is sent.** The upstream identifier
  index discards them, so a mistyped ISBN would otherwise return a different book, not an
  empty result.
- **A VÖBB branch (`AGB`) needs the second engine, `voebb.de`.** A KOBV record never says
  which branch holds a copy, so branch questions are answered by `voebb.de`, not by a
  filter on the union catalogue. `--at STABI,HU,AGB` runs two searches and prints three
  blocks; the two catalogues are never merged or matched against each other.
- **`--near` is offline.** It computes distance from coordinates already compiled into the
  binary (`--near 52.52,13.39` or `--near HU`); it does not geocode an address.

## Etiquette

Both `k2` and `voebb.de` serve `robots.txt: Disallow: /`. `blibs` is user-initiated
search, not crawling, so it follows the rules anyway:

- At most 6 in-flight requests per host, not user-configurable — and one for `voebb.de`,
  which answers overlapping requests in ten-second steps and is measurably faster asked
  one at a time.
- An honest `User-Agent` naming the tool and its repository.
- An on-disk response cache at `~/.cache/blibs/` (or `$XDG_CACHE_HOME`), 24 hours for
  bibliographic responses. Availability is never cached — "is it in right now" is asked
  live on every invocation, because a stale loan status is a wrong answer, not a fast one.
  `--no-cache` turns caching off entirely.
- No bulk harvesting. A request that implies mass extraction is out of scope.

## Development

```sh
cargo test                                 # all tests
cargo test parse::                         # one module
cargo clippy --all-targets -- -D warnings  # lint gate
cargo fmt --check
```

Fixtures for the HTML/JSON/MARCXML parsers live under `tests/fixtures/<engine>/`.

Architecture:

```
cli            → argument parsing, flag validation, engine choice, exit codes
render         → human (default) and JSON renderers over the same domain types
http            → the only I/O seam: Fetch trait, retry/backoff, per-host cap, cache
engine/kobv/    → SRU (x-pquery), portal availability JSON; composes http, interprets nothing
engine/voebb/   → session form, search POST, record page; composes the same http
model           → Record, RecordId, Holding, Item, Availability, SearchResult, Engine
select          → client-side sort/filter, and the "my libraries" match
error           → one error enum; every variant carries actionable remediation text
```

## Licence

GPL-3.0. See [`LICENSE`](LICENSE).
