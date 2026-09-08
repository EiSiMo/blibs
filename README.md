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
- **No holdings runs for journals.** The service answers one traffic light for the
  *title*. Where it lists the copies of a serial it does name the range each covers, and
  `blibs` prints them, one line and one light per copy:

  ```
  1911,643(24.Dez.) - 1920,134(13.März); 19…  Fremdsignatur M 038  available
  1911,643(24.Dez.) - 1934,77(31.März)        Ztg 1621 MR       available
  ```

  What no answer contains is the holdings *run* — which volumes a library has altogether —
  so a note on every serial says to ask the library's own catalogue or the ZDB.

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
blibs show almatuudk_BV021739966 --at UDK,ZLB
blibs libraries --find grimm
blibs libraries --branches
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
$ blibs search "Der Vorleser" --at HU,AGB --limit 3

HU Berlin · 35 results · showing 3

  ●  Der Vorleser                          Schlink, Bernhard     2019   almahu_BV047459758
       ZwB Naturwissenschaften, Lieblingsbüc…  S344 V9(.019)        available
  ●  Bernhard Schlink: Der Vorleser        Heigenmoser, Manfred  2005   almahu_BV020008568
       ZwB Germanistik/Skandinavistik, UG -…   GN 8896 H465         available
  ●  Erläuterungen zu Bernhard Schlink:…   Möckel, Magret        2003   almahu_BV017167903
       ZwB Germanistik/Skandinavistik, UG -…   GN 8896 M693(2)      available

AGB (VÖBB) · 137 results · showing 3

  ●  Der Vorleser                        Schlink, Bernhard   2012   voebb_SAK34846185
  ?  Der Vorleser                        Schlink, Bernhard   2012   voebb_SAK34672596
  ○  Der Vorleser : Roman                Schlink, Bernhard   2002   voebb_SAK13363539
       ZLB: Amerik…  L 248 Schlin 50 p    on loan               Standardausleihe - Vormerkung möglich

●  available      ○  on loan      ?  status not confirmed

note: this is an electronic title (Link zur Onleihe): it has no copies on a shelf, and its
      loan status is the one its lending link states — "Zugang zum Titel erhalten Sie
      hier. (Das Medium ist verfügbar / Ausleihe keine Vormerkung möglich)"
note: this is an electronic title (Link zu Overdrive): it has no copies on a shelf, and
      its lending link states no loan status this tool can read — "Zugang zum Titel
      erhalten Sie hier."
```

One block per location, in the order given to `--at`, and a KOBV institution reads the
same way. Each block's heading carries that location's true hit count (`137 results`), not
the count of the combined search; `showing N` appears whenever the block prints fewer than
that. A record can appear in more than one block if more than one of your locations holds
it — that is intentional, not a duplicate. Under each hit stand the copies **of that
location**: where they are, their shelfmark, their own status and what may be done with
them. `--no-availability` is the flag that turns those lines off.

The legend names only the symbols that occurred. `?` is one of them and is common: it
means the service was asked and stated nothing, which is never the same as "not there".

### Without `--at`: one flat line per hit

```
$ blibs search Kafka Prozess --limit 4

774 results for Kafka Prozess · showing 1-4

  1 ●  Der Prozess                       Einem, Gottfried von  2019  kobvindex_ZLB34296964
  2 ●  Der Prozess                       Einem, Gottfried von  2019  kobvindex_ZLB34302416
  3 ●  Der Prozeß                        Kafka, Franz          2014  kobvindex_ZLB00307271
  4 ?  Gert Westphal liest Franz Kafka   Kafka, Franz          2018  kobvindex_ZLB34175029

●  available somewhere      ?  status not confirmed
```

No location to group by, so no item lines here — run `show` on a record id for those. The
heading echoes the query as it was searched, field flags included (`133 results for
title: Process, author: Kafka`), because the count belongs to all of it. Output fits the
terminal down to 40 columns and uses the room it has above that.

### `show`: one title in full

```
$ blibs show almatuudk_BV021739966 --at UDK,ZLB

Das Schloss

  Haneke, Michael (1942-)           fmd              GND 119035979
  Kafka, Franz (1883-1924)          author           GND 118559230
  Stibr, Jiri (1937-)               cinematographer  GND 141021578
  Mühe, Ulrich (1953-2007)          actor            GND 121601706
  Lothar, Susanne (1960-2012)       actor            GND 123299470
  and 3 more

  Published    Absolut Medien, Berlin, 2004
  Extent       1 DVD-Video (ca. 123 Min. + 11 Min. Extras)
  Language     German
  Format       Video
  ISBN         389848761X
  Subjects     Spielfilm · Vermessungsingenieur · Dorf · Obrigkeit

Holdings

  ● UdK Berlin — Universität der Künste Berlin, Universitätsbibliothek
      UdK Universitätsbibliothek, Mediathek       SK 7871           available
  ● Berlin VÖBB/ZLB — Verbund der Öffentlichen Bibliotheken Berlins - VÖBB
    · 3 of 5 available
      Amerika-Gedenkbibliothek (AGB)              Film 10 Hane 8:DVD.Video  available
      Pankow / Kurt Tucholsky Bibliothek          DVD 2345          available
      Treptow-Köpenick / Mittelpunktbibliothek…   Spielfilm Schlos  available
      Amerika-Gedenkbibliothek (AGB)              Film 10 Hane 8 a:DVD.Video  on loan
      Neukölln / Helene-Nathan-Bibliothek         Spielfilm Schlos  on loan

  a copy on loan carries no due date here — return dates and holds are only in the
  library's own catalogue, behind a patron login

  also at: BHT

almatuudk_BV021739966
```

Libraries named in `--at` are listed first, in that order; everything else is one
`also at:` line. Without `--at`, every holding is simply listed and nothing is marked. A
role the record spells out is printed as it words it; where it states only the MARC
relator code, that code is printed instead of nothing (`fmd`).

Naming a **branch** in `--at` narrows the copies as well as the order: the light, the
`n of m available` count and the JSON's `at[].status` then answer "is it in *there*", and
the copies of the same library standing elsewhere are summarised in one line beneath.

## For agents

`--json` prints one JSON document on stdout and nothing else; errors go to stderr, also as
JSON. A shortened `search` document:

```json
{
  "query": { "terms": "Vorleser", "pqf": "@and @attr 1=1016 \"Vorleser\" @attr 1=1044 DE-11" },
  "total": 45,
  "shown": 4,
  "page": 1,
  "limit": 2,
  "sort": { "by": "relevance", "scope": "fetched" },
  "window": { "fetched": 9, "after_filter": 9, "filtered": false },
  "engines": ["kobv", "voebb"],
  "at": [
    { "key": "HU", "given": "HU", "isil": "DE-11", "branch": null, "engine": "kobv",
      "total": 45, "records": ["almahu_BV047459758", "almahu_BV014000689"] },
    { "key": "AGB", "given": "AGB", "isil": "DE-609", "branch": "SIG00036",
      "engine": "voebb", "total": 137,
      "records": ["voebb_SAK34846185", "voebb_SAK34672596"] }
  ],
  "availability": "fetched",
  "notes": [
    { "kind": "voebb_online_only", "message": "this is an electronic title (Link zur Onleihe): …",
      "records": ["voebb_SAK34846185"] }
  ],
  "records": [
    {
      "id": "almahu_BV047459758",
      "engine": "kobv",
      "source": "almahu",
      "local_id": "BV047459758",
      "title": "Der Vorleser",
      "subtitle": "Roman",
      "year": 2019,
      "format": "book",
      "online": false,
      "holdings": [
        {
          "isil": "DE-11",
          "alias": "HU",
          "library": "Humboldt-Universität zu Berlin, Universitätsbibliothek, …",
          "short_name": "HU Berlin",
          "mine": true,
          "summary": "available",
          "items": [
            { "location": "ZwB Germanistik/Skandinavistik, UG", "branch": "HUB00043",
              "branch_name": "Zweigbibliothek Germanistik/Skandinavistik",
              "call_number": "GM 5000 S345 V9", "volume": null,
              "status": "available", "order_option": null }
          ]
        }
      ]
    }
  ]
}
```

**Stability rule:** fields may be added over time; renaming or removing one is a breaking
change.

### Which question `--at` answers

With `--at`, **`at[].status` is the answer to "is it in *there*"**. It is the only field
narrowed to the location — and, for a branch, to that branch's copies.

`holdings[].summary` is a different statement: it is what the catalogue says about the
**whole institution**. For a VÖBB copy that institution is `DE-609`, the entire network of
97 branches. So

```
$ blibs show voebb_SAK13363539 --at AGB     →  ○ … on loan
$ blibs show voebb_SAK13363539 --at AGB --json
   .at[0].status          →  "unavailable"
   .record.holdings[0]    →  { "mine": true, "summary": "available", … }
```

is not a contradiction: the copy at the Amerika-Gedenkbibliothek is out, and somewhere in
the network a copy is in. `mine: true` says the location was matched, not that it is
available. The pipeline for "can I get it at my library" is `.at[] | .status`; use
`.record.holdings[]` when you want the institution-wide picture, and `items[].branch` to
see the copies branch by branch.

### The document's fields

- `query.pqf` is the query as it was sent, and it is `null` when there was no single one:
  when `--at` names two or more KOBV institutions each is searched separately, and when
  only `voebb` ran there is no PQF at all.
- `window` is `{ fetched, after_filter, filtered }`, plus `undelivered` and
  `before_available` when they apply — how many records the query returned, how many
  survived a client-side filter such as `--format`, and whether such a filter was set at
  all. Without the first two, "nothing found" and "nothing in this window" both look like
  an empty result; `filtered` is always present because `after_filter == fetched` is true
  both when no filter ran and when one matched everything.
- `window.undelivered` counts records the SRU envelope announced and did not deliver — a
  surrogate diagnostic in place of a record. It is **absent when zero**, so its presence
  is the signal. The `record_undelivered` note names what happened.
- `window.before_available` is there only when `--available` ran, and says how many of the
  displayed records it judged. The survivors are `shown`, so the hidden ones are the
  difference; its absence is how a page that was always this short is told apart from one
  the filter thinned out. `at[].total` is untouched by the filter — it stays that
  location's true hit count, and only `at[].records` is filtered.
- **With `--format`, `--language` or any `--sort` but relevance, the window is the whole
  addressable set.** It is one anchored block of 50 records and `--page` walks the
  *matches inside it*, so pages partition those matches and no page reaches past the
  block. Without any of them `--page` steps the result itself, as `(page - 1) * limit + 1`.
- `at[]` gives, per `--at` location, `given` (the word the user typed) beside `key` (the
  canonical short name), the resolved ISIL and branch, which engine answered, and that
  location's true hit count. An agent reconstructs the grouped view from this and
  `holdings[].isil` / `items[].branch`; `records` stays record-centric, with each record
  listed exactly once even if it would appear in several blocks.
- `availability` is `"fetched"` or `"skipped"` — whether the status service was asked at
  all, so an empty `items` list is never ambiguous.
- `total` is the KOBV hit count, and it is `null` in three cases: only `voebb` ran
  (voebb.de states no figure for the whole network, only per branch), `--at` named two or
  more KOBV institutions (each is asked separately and a sum would double-count a record
  held in several houses), or the catalogue stated none. `at[].total` is the number to
  trust — and it is itself `null` for a branch of a KOBV institution, which the catalogue
  cannot count (`branch_from_copies`).
- `limit` is **per location**, so `shown` can exceed it once `--at` names more than one:
  `--at HU,AGB --limit 3` gives `limit: 3` and `shown: 6`. It is the count of *distinct*
  records, though — a record standing under two headings is listed once — so it is not
  always the sum of the blocks. A KOBV window is fetched a few records wider than `limit`
  so a record the catalogue delivered twice can be dropped and replaced; at `--limit 50`
  there is no room left for that, and `duplicate_records_dropped` explains the short page.
- `authors[].role` is the relator term as the record words it (`Verfasser/in`, `Übers.`),
  `authors[].role_code` the MARC relator code (`aut`, `trl`). Neither vocabulary is
  closed — the data carries German extensions no LoC list contains — and `voebb` never
  states a code at all, so `null` there means "not stated in this catalogue".

### `notes[]`

Limitations that are not errors. Each is `{ kind, message }`, plus `records[]` where the
note belongs to particular records. **Switch on `kind`, never on the wording.** The whole
field is *absent* when there is nothing to say, rather than an empty array.

| `kind` | `search` | `show` | What it says |
| --- | :-: | :-: | --- |
| `record_undelivered` | ✓ | | the envelope announced a record and sent a diagnostic |
| `record_schema_unknown` | ✓ | | a record arrived in a schema this tool cannot read |
| `window_filter_empty` | ✓ | | `--format`/`--language` matched none of the fetched records |
| `duplicate_records_dropped` | ✓ | | the catalogue delivered one record twice in this window |
| `result_order_unstable` | ✓ | | from page two on: consecutive pages may overlap or skip |
| `availability_filter_unstated` | ✓ | | `--available` hid records the service says nothing about |
| `voebb_free_terms_as_title` | ✓ | | free words next to a field flag were searched as a title |
| `voebb_query_truncated` | ✓ | | the query needed more rows than voebb.de's form has |
| `voebb_branch_not_listed` | ✓ | | the branch facet does not list this branch for this search |
| `voebb_no_hits_in_network` | ✓ | | nothing anywhere in the VÖBB network — not the branch's doing |
| `availability_not_stated` | ✓ | ✓ | `hasAvailability: false`; the copies come from the record alone |
| `availability_unknown_status` | ✓ | ✓ | a traffic light was missing or unknown, so nothing was guessed |
| `availability_matched_by_name` | ✓ | ✓ | copy blocks were matched by library name, not by order |
| `availability_match_conflict` | ✓ | ✓ | order and name disagree about a block; order won |
| `availability_status_conflict` | ✓ | ✓ | a voebb copy's two status signals disagree; the worse one won |
| `holding_without_isil` | ✓ | ✓ | a copy block matched no ISIL and is shown under the portal's name |
| `voebb_multivolume` | ✓ | ✓ | the copies belong to the volumes, which are records of their own |
| `voebb_online_only` | ✓ | ✓ | a lending-platform title; its state was read from the link text |
| `voebb_online_state_unstated` | ✓ | ✓ | the same, with a link that states no status this tool knows |
| `branch_from_copies` | ✓ | ✓ | a KOBV branch was sieved from the copies; no total, never absence |
| `branch_needs_copies` | ✓ | ✓ | `--no-availability` left no copies to read the branch from |
| `location_other_catalogue` | | ✓ | `--at` names a location the other catalogue answers for |
| `serial_volumes_unknown` | | ✓ | one status for the title; which volumes are held is not derivable |
| `loan_without_due_date` | | ✓ | a copy is out and no return date exists without a patron login |

### `show` and `libraries`

`show` answers with a document of its own:

```json
{
  "record": { "id": "voebb_SAK13363539", "title": "Der Vorleser", "…": "…" },
  "at": [ { "key": "AGB", "given": "AGB", "isil": "DE-609", "branch": "SIG00036",
            "engine": "voebb", "status": "unavailable" } ],
  "availability": "fetched"
}
```

`record` is `null` when the id matched nothing, so even a miss says whether availability
was asked for. `at[]` is absent without `--at`, and `availability` and `notes` mean what
they mean above.

`libraries` prints a flat array of entries, each with a `type` of `"institution"` or
`"branch"`. A house carries its own branches in `branches[]`; a branch that stands at the
top level — every entry under `--branches`, and under `--find` a branch whose house did
not match itself — carries `parent` and a `search` object naming the key to put in `--at`,
the engine that answers it, and a `limitation` (`"branch_from_copies"`, or `null` for a
VÖBB branch, whose block is complete). With `--near`, each entry gains `distance_km`: a
straight-line float from coordinates compiled into the binary.

### Exit codes

| Code | Meaning |
| --- | --- |
| 0 | found at least one hit |
| 1 | searched and found nothing; also an unknown record id, or `libraries --find`/`--near` matching nothing |
| 2 | usage error — nothing was sent; includes a library shortcode that does not exist |
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
    "hint": "did you mean STABI? run `blibs libraries --find <name>` to look one up, or `blibs libraries --branches` to list every branch"
  }
}
```

`message` is always a single line and never carries a usage block; `hint` says what to do
next.

## Good to know

- **`--author` is always searched as a word list, never a phrase.** The author index holds
  authority forms: as a phrase, `"Kafka, Franz"` finds 2914 records and `"Franz Kafka"`
  finds 40, so a natural name is split into words instead.
- **`--year` means "was running in this year", not "was published in this year".** For a
  journal that ran 1923–1991, `--year 1960` matches and `--year 1922` does not.
- **`--sort`, `--format` and `--language` only see the fetched window**, not the whole
  result — the catalogue itself cannot sort or filter by material type or language, and
  all three therefore anchor the window as described above. A short result after
  `--format` is stated as a window effect, never as "none exist".
- **The catalogue does not order a result stably.** Two identical requests return
  different records in a different order — measured against `sru.kobv.de/k2` directly. So
  consecutive pages can overlap or leave records out, and from page two on a
  `result_order_unstable` note says so. The cache hides this: repeated identical calls
  look stable until `--no-cache`.
- **`--available` thins the page out, it does not reload.** The status is only asked about
  for the records that are shown, so ten hits can come back as four — use a larger
  `--limit` to see more. Reference stock does not count as available, and a record the
  catalogue states no status for drops out too; the footer says how many went and how many
  of those were never answered. With several `--at` locations it acts per block. Combining
  it with `--no-availability` is a usage error, as is `--sort availability` with it —
  there would be nothing to judge on.
- **Subject headings are mixed-language and untranslated.** `--subject Recht` and
  `--subject law` are different searches; both can return hits.
- **ISBN check digits are verified before anything is sent.** The upstream identifier
  index discards them, so a mistyped ISBN would otherwise return a different book, not an
  empty result. An ISSN is accepted the same way.
- **Language codes are the bibliographic ones.** `--language ger`, not `deu` and not `de`;
  the terminology code is refused with the bibliographic one named.
- **A VÖBB branch (`AGB`) needs the second engine, `voebb.de`.** A KOBV record never says
  which branch holds a copy, so branch questions are answered by `voebb.de`, not by a
  filter on the union catalogue. `--at STABI,HU,AGB` runs two searches and prints three
  blocks; the two catalogues are never merged or matched against each other. A branch of
  any *other* institution is sieved from the copies instead: that block states no total,
  can be shorter than `--limit`, and never proves that the branch holds nothing.
- **`--near` is offline.** It computes distance from coordinates already compiled into the
  binary (`--near 52.52,13.39` or `--near HU`); it does not geocode an address. It ranks
  branches as well as houses.

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
cli           → argument parsing, flag validation, engine choice, exit codes
render        → human (default) and JSON renderers over the same domain types
http          → the only I/O seam: Fetch trait, retry/backoff, per-host cap, cache
engine/kobv/  → SRU (x-pquery), portal availability JSON; composes http, interprets nothing
engine/voebb/ → session form, search POST, record page; composes the same http
model         → Record, RecordId, Holding, Item, Availability, SearchResult, Engine
select        → client-side sort/filter, and the "my libraries" match
error         → one error enum; every variant carries actionable remediation text
```

## Licence

GPL-3.0. See [`LICENSE`](LICENSE).
