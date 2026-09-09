# tests/fixtures

Saved HTTP responses for parser unit tests. Nothing here does I/O — `engine/*/parse`
consumes these files directly, `engine/*/client` never runs against them.

```
kobv/sru/*.xml           SRU searchRetrieve responses (recordSchema=marcxml)
kobv/availability/*.json portal.kobv.de getAvailability responses
voebb/                   built by a separate agent (voebb.de session/results/detail fixtures)
schema/                  JSON schema snapshots for SearchResult (empty for now, Phase 1+)
```

All 28 original `sru_*.xml` files were downloaded 2026-09-04 with 50 records each
(`maximumRecords=50`, CQL `query=`) and have since been shrunk to the handful of
`<zs:record>` elements that carry a documented trap, by pure text surgery on the
`<zs:record>…</zs:record>` blocks (no XML reserialization, so comments/whitespace/entities
inside kept records are byte-identical to the original download). The envelope
(`numberOfRecords`, `nextRecordPosition`, `echoedSearchRetrieveRequest`) is **left exactly
as downloaded** — it now overstates the record count for every trimmed file; that mismatch
is the point (parsers must never infer record count from the envelope for a *test* file,
only trust what is actually enumerated). `recordPosition` values on kept records were never
renumbered.

Budget: `tests/fixtures` must stay ≤ 600 KB (`du -sh`). This forced two deviations from the
general "3–5 records per file" shrink target, both noted in the tables below:

- Most files that had no assigned special-case ID were shrunk to their **single smallest**
  remaining record — a representative sample of that material type, not a specific trap.
- `nonlatin_ru.xml` was asked to carry "2 records", read as evidence for 880 (non-Latin
  alternate-script) handling; the original 50 records contained **only one** record with an
  `880` field anywhere (`gbv_730267962`, 6 such fields) — it was cut to stay in budget, so
  this file now carries only the explicitly required `almahu_BV010644426` and has **no**
  `880` example of its own (arabic.xml and nonlatin_he.xml still cover that trap).

**Not found anywhere**: `edocfu_9960819782502883` ("Serial 1923–1991") does not occur in
any of the 28 original SRU fixtures — grepped across all of them before shrinking. Not
guessed, not fabricated; flagging per instructions.

Refetching: base `https://sru.kobv.de/k2?operation=searchRetrieve&version=2.0`,
`recordSchema=marcxml`, `UA='blibs/0.1 (+https://github.com/EiSiMo/blibs)'`, `--compressed`,
one request at a time. See `plan/scraping.md` § Fixtures for the full recipe set and § A.4a
for the `x-pquery`/PQF attribute table.

## kobv/sru — search results (originally 50 records each, downloaded 2026-09-04)

| File | Original query | Kept / 50 | Evidences |
| --- | --- | --- | --- |
| `arabic.xml` | `dc.title="al-Qahira"` | 5 | `gbv_660841622` 700$t (translator added entry with title); `gbv_1603494723` 041 multilingual tur/eng/tam; `almafu_BV026062583` 264$c Islamic/Hijri calendar date "1413 [1992]" **and** an 880 alternate-script field; `gbv_102743228X` 264$c French Republican calendar "Le 27 Nivôse An VIII"; `gbv_014140950` 110 corporate author with $b/$g |
| `av_dvd.xml` | `dc.title="DVD-Video"` | 1 | `gbv_519092074` 245 with $n/$p interleaved (two $n, one $p between them) |
| `av_sound.xml` | `dc.title="Compact Disc"` | 1 | `kobvindex_ZLB12845080` volume of a multi-part series |
| `ebook.xml` | `dc.title="Springer eBook Collection"` | 1 | generic sample, no assigned trap |
| `ebook2.xml` | `Online-Ressource Volltext` | 1 | `kobvindex_ZLB15770715` Leader/06=`m` (electronic resource) with 007 `cr` |
| `english.xml` | `dc.title="Machine Learning"` | 1 | generic sample, no assigned trap |
| `festschrift.xml` | `Festschrift` | 2 | `almahu_9950038349602882` 008 is 34 chars (not the usual 40, offset shift); `kobvindex_LDAz01080` corrupted/shifted leader |
| `french.xml` | `dc.title="Histoire de la litterature"` | 1 | generic sample, no assigned trap |
| `journal.xml` | `dc.title="Zeitschrift fuer Soziologie"` | 2 | `b3kat_BV002580991` five 264 fields with $3 period qualifiers ("anfangs", "-1999", …); `kobvindex_MFN13563` 008 truncated to 34 chars for a continuing resource |
| `journal2.xml` | `dc.title="Jahresbericht"` | 1 | `gbv_130857440` twelve repeated 264 fields (century of publisher/place history) |
| `kids.xml` | `dc.title="Bilderbuch"` | 3 | kept by **position**, not content: recordPosition 48 (ordinary record), 49 (surrogate diagnostic — `recordSchema=info:srw/schema/1/diagnostics-v1.1`, uri 1/63, *inside* the record list), 50 (`kobvindex_VBRD-schursalnatoih23lanmed20eidi129`, a VÖBB-style local id surfaced through the KOBV index) |
| `map.xml` | `dc.title="Topographische Karte"` | 1 | generic sample, no assigned trap |
| `map2.xml` | `dc.title="Stadtplan"` | 1 | generic sample, no assigned trap |
| `microform.xml` | `dc.title="Mikrofiche"` | 1 | generic sample, no assigned trap |
| `mono_kafka.xml` | `dc.title="Prozess" and dc.creator="Kafka"` | 2 | `b3kat_BV044513648` 245$a carries raw C1 controls U+0098/U+009C bracketing "Der"; `almahu_BV019771323` five 924 holdings, two 852 (portal-only, never parsed from SRU), three 689 with $s |
| `multivol.xml` | `dc.title="Gesammelte Werke"` | 1 | `almatuudk_9922218477402884` double-encoded UTF-8 in a 246 (Č → bytes 0xC4 0x8D → misread as Ä + U+008D) |
| `newspaper.xml` | `dc.title="Tageszeitung"` | 1 | `kobvindex_SLB42822` (= original first record): inline `<!--…-->` comments from a metaproxy MARC-directory warning, 008 all-`\|`, 041=ger, doubled 245$n |
| `nonlatin_cjk.xml` | `dc.title="Zhongguo"` | 2 | `almafu_9958130249502883`, `gbv_815486464` — both carry 880 alternate-script (CJK) fields |
| `nonlatin_he.xml` | `dc.title="Talmud"` | 1 | `gbv_1944932879` 880 alternate-script (Hebrew) field; 264$c Hebrew-calendar year "5733 [1972 oder 1973]" |
| `nonlatin_ru.xml` | `Dostoevskij` | 1 | `almahu_BV010644426` six identical 700 added entries. **No 880 example** — see deviation note above |
| `oldprint.xml` | `dc.date="1750"` | 1 | `kobvindex_MFN19797` 100$a with a doubled-angle-bracket nobiliary particle `<<de>>` |
| `oldprint2.xml` | `dc.date="1680"` | 1 | generic sample, no assigned trap |
| `score.xml` | `dc.title="Klavierauszug"` | 1 | `kobvindex_SLB24672` two 245 fields, the first without a $a |
| `score2.xml` | `dc.title="Streichquartett"` | 1 | generic sample, no assigned trap |
| `series.xml` | `dc.title="Schriftenreihe"` | 1 | generic sample, no assigned trap |
| `thesis.xml` | `Inauguraldissertation` | 1 | generic sample, no assigned trap |
| `thesis2.xml` | `Hochschulschrift Dissertation Universitaet` | 1 | generic sample, no assigned trap |
| `video_game.xml` | `dc.title="Computerspiel"` | 1 | generic sample, no assigned trap |

Refetch any of the above (get the full 50 back) with, e.g.:

```bash
curl -sS -A "$UA" --compressed -G "$S" \
  -d operation=searchRetrieve -d version=2.0 -d maximumRecords=50 -d recordSchema=marcxml \
  --data-urlencode 'query=dc.title="al-Qahira"' \
  > arabic.xml
```

## kobv/sru — new fixtures (fetched 2026-09-06, via `x-pquery`/PQF, see § A.4a)

| File | Request | Result |
| --- | --- | --- |
| `empty.xml` | `x-pquery=@attr 1=1016 "zzzqqxnonexistentterm"`, `maximumRecords=10` | `numberOfRecords=0`, no records, no diagnostics — the clean "no hits" case |
| `diagnostic.xml` | `query=dc.nonexistentindex="x"` (CQL, per plan §Fixtures) | top-level diagnostic `1/16` "Unsupported index" |
| `record.xml` | `x-pquery=@attr 1=12 almafu_BV008885798`, `maximumRecords=1` | `numberOfRecords=1`, one full MARC record (the reference Kafka *Prozess* record used throughout `plan/`) |
| `count.xml` | `x-pquery=@attr 1=4 "Harry Potter"`, `maximumRecords=0` | `numberOfRecords=3718`, zero records returned — count-only request |
| `unknown_id.xml` | `x-pquery=@attr 1=12 almafu_BV000000000`, `maximumRecords=1` | `numberOfRecords=0` — well-formed id, no such record: empty, not an error |
| `filtered.xml` | `x-pquery=@and @attr 1=4 "Harry Potter" @attr 1=1044 DE-11`, `maximumRecords=3` | `numberOfRecords=230` (matches the A.4a measurement exactly), 3 records — evidence for the `1044` ISIL filter |
| `pqf_diag_truncation.xml` | `x-pquery=@attr 1=4 @attr 5=1 Harr` | diagnostic `1/48` "Right truncation not supported" |
| `out_of_range.xml` | `x-pquery=@attr 1=1016 "Kafka"`, `startRecord=24996`, `maximumRecords=5` (fetched 2026-09-07) | `numberOfRecords=13410` **and** diagnostic `1/61` "First record position out of range" — the count is stated and the window is still refused, so this is a rejection, not an empty result |
| `not_xml.html` | **synthetic, not fetched** | minimal `<html><body><h1>502 Proxy Error</h1></body></html>` — stands in for "HTTP 200 but not XML" |

```bash
UA='blibs/0.1 (+https://github.com/EiSiMo/blibs)'
S=https://sru.kobv.de/k2
curl -sS -A "$UA" --compressed -G "$S" \
  -d operation=searchRetrieve -d version=2.0 -d maximumRecords=3 -d recordSchema=marcxml \
  --data-urlencode 'x-pquery=@and @attr 1=4 "Harry Potter" @attr 1=1044 DE-11' \
  > filtered.xml
```

## kobv/availability — getAvailability responses (2026-09-04)

| File | `availability_id` | Evidences |
| --- | --- | --- |
| `journal.json` | `DE-B496;BV002580991,DE-Bo133;BV002580991,DE-Po75;BV002580991` | multi-institution holding for a serial |
| `mixed.json` | `DE-83;BV041830956,DE-B170;BV041830956,DE-188;BV041830956,DE-11;BV041830956,DE-B1533;BV041830956,DE-1;1608897087` | positional vs. `portal_name` matching agree ("Weg 1 == Weg 2") |
| `newspaper.json` | `DE-1;166681512` | 5-column shelf table (newspapers get an extra column vs. books) |
| `public.json` | `DE-B1533;BV049617543,DE-634;BV049617543,DE-521;BV049617543` | public-library holding, status rendered flat ("black") rather than per-item |
| `no_items.json` | *not recorded* — response shows `bibids=SIG00057` ("Kammergericht", PPN 518345920) | two `tr.avail-item` rows with an empty Call Number cell and a "Library" location placeholder — no shelfmark stated at all |
| `reference.json` | *not recorded* — response shows `bibids=BIB000000017` ("Stadtmuseum Berlin") | `availability-status-yellow` → `reference` (non-circulating) status |
| `comma_prefix.json` | **synthetic**, built by hand from `reference.json`'s shape | the location link text is `, Unter den Linden` — a bare join comma with an empty house name in front of it, reproducing the live response `curl` showed for `Berliner Zeitung` at STABI (`plan/feedback_round_3.md` §3.2: `blibs search --title "Berliner Zeitung" --at STABI`, `availability_id=DE-1;130560987,`, cell text `<a href="…&bibids=BIB000000004">, Unter den Linden</a>`). Not refetched with `curl` because the live response is 4.9 KB and this trap is two attributes of it; every other value (`bibids`, ISIL `DE-1`, portal name `Stabi Berlin`, call number `2"@Ztg 5011;Erg-Bd.`, yellow/`reference` status) is copied from that measurement. |

`no_items.json` and `reference.json` predate this pass and their exact `availability_id` was
not preserved anywhere in `plan/`; the institution/PPN visible in the saved HTML is the only
trace. Regenerating them means finding a current record held at that institution with,
respectively, no shelfmark and a "reference" (yellow) item and calling:

```bash
curl -sS -A "$UA" --compressed -G "$P/AJAX/JSON" \
  --data-urlencode 'method=getAvailability' \
  --data-urlencode 'availability_id=<ISIL>;<local_id>,' \
  > kobv/availability/<name>.json
```

with `P=https://portal.kobv.de`.
