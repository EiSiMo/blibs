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
- **No holds — and a return date only where the catalogue prints one.** A hold queue lives
  behind a library account, and `blibs` signs in nowhere. A due date is different:
  `voebb.de` writes one beside the status of a copy that is out, so those lines read
  `on loan, due 15 Sep 2026`. KOBV states none at all, and a copy that is out with nothing
  said about it stays a bare `on loan` — never a guessed date.
- **No holdings runs for journals, with one exception.** The availability service answers
  one traffic light for the *title*. Where it lists the copies of a serial it does name
  the range each covers, and `blibs` prints them, one line and one light per copy:

  ```
  [1.]1872,1.Jan. - 68.1939,51…  Fremdsignatur M 208          available
  1898,11.Dez. - 1908,Dez.       Ztg 9018 MR                  available
  ```

  What that still does not give is the holdings *run* — which volumes a library has
  altogether — and a note on such a record says to ask the library's own catalogue or the
  ZDB. The exception is `voebb.de`: for its serials and newspapers it states the run as
  one line of prose, which `blibs` carries as `holdings[].holdings_statement` and prints
  above the copies — **whole**, because the `Standort:` and `Signatur:` inside it are free
  text and not a grammar. Not every such record has one, and no KOBV record does.

## Installation

```sh
cargo install blibs
# or, from a clone:
cargo install --path .
# or, for a local build:
cargo build --release
```

Requires Rust 1.88 or later (edition 2024) — let-chains, which both blibs and `scraper` use.

The crate also builds a library target, because the binary and the integration tests are
built from it. **It is not a public API**: nothing under `blibs::` carries a stability
guarantee, and any release may rename or remove any of it. What is stable is what this
file documents — the command line, the JSON schema, the `notes[].kind` vocabulary and the
exit codes.

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
$ blibs search "Der Herr der Ringe" --at HU,AGB --limit 3

HU Berlin · N results · showing 3

  ●  Der Herr der Ringe            Jackson, Peter           2004   almafu_BV036622058
       ZB Grimm-Zentrum, 7. OG - Mediathek  2025 DVDM 21         available
  ●  Der Hobbit und Der Herr der…  Rittstieg, Jana          2015   almahu_9948326315502882
       Online-Zugriff · nur im Netz der HU                       available
  ●  From Page to Screen / Vom B…  Almagro-Jiménez, Manuel  2020   almahu_9950338104802882
       1 copy                                                    available

AGB (VÖBB) · N results · showing 3

  ?  Der Herr der Ringe                  Tolkien, J. R. R.          voebb_SAK11034413
  ?  Der Herr der Ringe                  Tolkien, J. R. R.   2003   voebb_SAK34704391
  ○  Der Herr der Ringe : die Chronik                        2022   voebb_SAK34954522
       ZLB: Amerik…  Th 722/79            on loan, due 15 Sep 2026  Standardausleihe - Vormerkung möglich

●  available      ○  on loan      ?  status not confirmed

note: voebb.de lists no copies for a multi-part work — the copies belong to its volumes,
      which are records of their own; search for the volume
      record: voebb_SAK11034413
note: an electronic title has no copies on a shelf, and the Link zu Overdrive row states
      no loan status this tool can read — "Zugang zum Titel erhalten Sie hier."
      record: voebb_SAK34704391
```

One block per location, in the order given to `--at`, and a KOBV institution reads the
same way. Each block's heading carries that location's true hit count, not the count of
the combined search; `showing 3` appears whenever the block prints fewer than that. The
hit counts stand as `N` here because the index grows and no two identical searches answer
alike. A record can appear in more than one block if more than one of your locations holds
it — that is intentional, not a duplicate. Under each hit stand the copies **of that
location**: where they are, their shelfmark, their own status and what may be done with
them. `--no-availability` is the flag that turns those lines off.

The legend names only the symbols that occurred. `?` is one of them and is common: it
means the service was asked and stated nothing, which is never the same as "not there".

Under each note stands the line that says **which** records it is about — `record: <id>`,
`4 records: …`, or `all 10 records` when it is every one of them. A note never spells its
records into the sentence, so two records with the same limitation print one paragraph and
not two; `notes[].records` in `--json` is the same list.

### Without `--at`: one flat line per hit

```
$ blibs search Kafka Prozess --limit 4

N results for Kafka Prozess · showing 1-4

  1 ●  Der Prozess                       Einem, Gottfried von  2019  kobvindex_ZLB34296964
  2 ●  Der Prozess                       Einem, Gottfried von  2019  kobvindex_ZLB34302416
  3 ●  Der Prozeß                        Kafka, Franz          2014  kobvindex_ZLB00307271
  4 ?  Gert Westphal liest Franz Kafka   Kafka, Franz          2018  kobvindex_ZLB34175029

●  available somewhere      ?  status not confirmed
```

No location to group by, so no item lines here — run `show` on a record id for those. The
heading echoes the query as it was searched, field flags included (`N results for
title: Process, author: Kafka`), because the count belongs to all of it. Output fits the
terminal down to 40 columns and uses the room it has above that. A shelfmark, a status and
a record id are never shortened — half a shelfmark finds no book, half a date is a
different deadline, and a cut id cannot be typed back into `show`. The columns that may be
cut give way for them instead: the title and author here, the location under a hit, and
each says so with an ellipsis.

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
      UdK Universitätsbibliothek, Med…  SK 7871                     available
  ● Berlin VÖBB/ZLB — Verbund der Öffentlichen Bibliotheken Berlins - VÖBB
    · 3 of 5 available
      Amerika-Gedenkbibliothek (AGB)    Film 10 Hane 8:DVD.Video    available
      Pankow / Kurt Tucholsky Bibliot…  DVD 2345                    available
      Treptow-Köpenick / Mittelpunktb…  Spielfilm Schlos            available
      Amerika-Gedenkbibliothek (AGB)    Film 10 Hane 8 a:DVD.Video  on loan
      Neukölln / Helene-Nathan-Biblio…  Spielfilm Schlos            on loan

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
    { "kind": "voebb_online_only",
      "message": "an electronic title has no copies on a shelf; the loan status shown is …",
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
          "local_id": "BV047459758",
          "mine": true,
          "summary": "available",
          "holdings_statement": null,
          "items": [
            { "location": "ZwB Germanistik/Skandinavistik, UG", "branch": "HUB00043",
              "branch_name": "Zweigbibliothek Germanistik/Skandinavistik",
              "call_number": "GM 5000 S345 V9", "volume": null,
              "status": "available", "due_date": null, "order_option": null }
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
- `items[].due_date` is the return date of a copy that is out, as **ISO-8601**
  (`"2026-09-22"`). Only `voebb.de` states one, so it is `null` throughout a KOBV answer
  and `null` for a copy that is in. It never decides `items[].status`, which comes from
  the traffic light alone; a date stated in a form this tool cannot read stays `null` and
  raises `voebb_due_date_unreadable` rather than being bent into shape.
- `holdings[].holdings_statement` is what a library says about its run **in prose**,
  verbatim and undivided — voebb.de's `Bestand` line for a serial or a newspaper
  (`"Bestand in ZLB: 1994/95,1 - 1998/99,17(22.Apr.) Mikrofilm Standort: BStB Signatur: A
  80 ZC 181 Beil.:Mikro"`). The `Standort:` and `Signatur:` inside it look like structure
  and are free text, so it is never split: a guessed shelfmark would be quoted as though
  the catalogue had stated one. Such a record normally has an **empty** `items[]` — the
  statement is where its holdings are, not a loss — and it is `null` for every KOBV
  holding, whose `924` fields arrive as items.

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
| `isbn_neighbours_dropped` | ✓ | | `--isbn`: records carrying a different ISBN were dropped |
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
| `voebb_no_copies_listed` | ✓ | ✓ | the item table is present and empty, and the page says nothing about why |
| `voebb_online_only` | ✓ | ✓ | a lending-platform title; its state was read from the link text |
| `voebb_online_state_unstated` | ✓ | ✓ | the same, with a link that states no status this tool knows |
| `voebb_online_url_only` | ✓ | ✓ | an electronic title reachable only by a plain URL; no loan state anywhere |
| `voebb_due_date_unreadable` | ✓ | ✓ | a return date was stated in a form this tool cannot read; none was guessed |
| `voebb_page_unreadable` | ✓ | ✓ | one record page changed shape: that record lost its copies, the answer stands |
| `branch_from_copies` | ✓ | ✓ | a KOBV branch was sieved from the copies; no total, never absence |
| `branch_needs_copies` | ✓ | ✓ | `--no-availability` left no copies to read the branch from |
| `location_key_ambiguous` | ✓ | ✓ | the `--at` key is claimed by several libraries; the first one answered |
| `location_past_the_last_result` | ✓ | | the window begins past that location's last result; its block is empty and states no total |
| `location_window_too_deep` | ✓ | | the page lies deeper than that catalogue can be paged; nothing was asked, nothing is said |
| `location_other_catalogue` | | ✓ | `--at` names a location the other catalogue answers for |
| `serial_volumes_unknown` | | ✓ | one status for the title, and the record states no run of its own |
| `loan_without_due_date` | | ✓ | a copy is out and states **no** date; for that copy only a patron login has one |

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
  authority forms (`Kafka, Franz`), so a phrase in the natural name order finds two orders
  of magnitude fewer records than the same name inverted — measured against the index
  itself, and a ratio rather than a count because the catalogue grows. A name is therefore
  split into words, which finds them written either way round.
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
- **An ISBN answers with that ISBN, or with nothing.** The check digit is verified before
  anything is sent, because the upstream identifier index discards it — measured, a search
  for `9783596294336` and for `…330` and `…331` returns the same 18 records, and so does
  the ISBN-10 form. That blindness also lets the index answer with records whose own `020`
  holds a different number, so those are dropped before anything is counted: the page can
  be shorter than the count above it, an `isbn_neighbours_dropped` note says by how much,
  and an answer in which *no* record carries the ISBN is exit 1 that says so rather than a
  page of somebody else's editions. A record that states no ISBN at all is kept — silence
  is not a different number. An ISSN is verified the same way and needs no sieve; there
  the check digit is significant upstream.
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
libraries     → the compiled-in list: resolves --at and a record's ISIL, and ranks by distance
error         → one error enum; every variant carries actionable remediation text
counts        → the English phrases for a count, shared by error and render
```

## Licence

MIT. See [`LICENSE`](LICENSE).
