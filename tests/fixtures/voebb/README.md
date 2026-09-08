# Fixtures: voebb.de (aDISWeb)

Alle Dateien sind Serverantworten, gezogen am **2026-09-06**. User-Agent bei jedem Abruf:
`blibs/0.1 (+https://github.com/EiSiMo/blibs)`, seriell, ≥ 1 s Pause, 24 curl-Aufrufe
insgesamt.

Die Antworten sind **gekürzt, nicht verändert**: aus 1,1 MB sind 384 KiB geworden, indem
*ganze Elemente* entfernt wurden — kein Zeichen des verbleibenden Markups ist umgeschrieben,
kein Attribut umsortiert. Was weg ist und warum, steht unten in § *Was gekürzt wurde*; das
Rezept ist `slim.py` daneben, mit dem sich eine frische Messung genauso eindampfen lässt.

Sitzungskennungen (`_3437h8…` im Formularpfad, `identity`, `requestCount`) stehen absichtlich
noch drin: sie sind nach Minuten ungültig und tragen kein Geheimnis. Der Parser muss sie
aus dem Dokument lesen, also gehören sie in die Fixture.

Die Wege, die diese Dateien erzeugt haben, stehen ausführlich in `plan/voebb.md`
§ *Gemessene Wege*; hier nur Herkunft und Beweislast.

## Eine Sitzung, sieben Seiten

`start.html` … `results_isbn.html` stammen aus **einer** Sitzung
(`_21g4ad7n7gpvxuzq2ryo9pn9w87rqtq7`), in dieser Reihenfolge. Deshalb ist an ihnen die
`requestCount`-Kette 1 → 2 → 3 → 4 → 5 → 6 ablesbar, und `identity` wechselt bei **jeder**
Antwort. Die Detailseiten sind je eine eigene Sitzung (siehe unten).

| Datei | Herkunft / Rezept | Was sie belegt |
| --- | --- | --- |
| `start.html` | `GET https://www.voebb.de/` mit `-L` und Cookie-Jar (3 HTTP-Hops: 301 → 302 → 200) | Sitzungseröffnung: Action-Pfad mit Sitzungskennung, die 9 versteckten Felder, `requestCount=1`, die Einfeldsuche `$Autosuggest` + `$Button`, der Suchbereich `$Select` (85 Optionen, hier auf die ersten fünf gekürzt), `$Button$0` = *Erweiterte Suche* |
| `results.html` | `POST <action>` mit den 9 Feldern aus `start.html` + `$Select=Bibliotheksbestand&$Autosuggest=Der Vorleser Schlink&$Button=Suchen` (303 → 200) | Trefferliste: `div#R06 p.info` mit `Treffer: 71`, 22 Zeilen `li.rList_li`, Satz-ID `SAK…` im Titel-Link, Verfügbarkeitsampel je Zeile, Medienart als `alt`, **vollständiger Facettenbaum** `div#PTL1_tree_1` mit 7 Rubriken, `$Button$2`/`$Button$3` = *Filtern*, Sortierknöpfe `$Button$4…$8`, Blätter-Toolbar (`$Toolbar_1` und `_2` `disabled`, `_3`/`_4` aktiv) |
| `results_filtered.html` | `POST` mit dem Formularzustand von `results.html` + `$CbTree_text=sub-PTL1_tree_1_90&$Button$2=pressed&source=$B&focus=$$GFBO_11` | **Hausfacette wirkt upstream**: 71 → 35, genau die Facettenzahl. Rubrik *Aktive/r Filter* an der Baumspitze (`h4#stat-PTL1_tree_1_0`) mit `checked="on"` und dem Label `Bibliothek: "ZLB: Amerika-Gedenkbibliothek (AGB)"`. Und: **die Baumnummern haben sich verschoben** — dieselbe AGB heißt hier `sub-PTL1_tree_1_92` statt `_90`, weil oben eine Rubrik dazugekommen ist. Der Beweis, dass `PTL1_tree_1_<n>` keine stabile Kennung ist |
| `results_filtered_page2.html` | `POST` mit dem Zustand von `results_filtered.html` + `$Toolbar=3` (Wert 3 = Knopf *nächster*) | Blättern: die Positionen in `div.rList_num` laufen absolut weiter (23 … 35), Gesamtzahl bleibt 35, `$Toolbar_3` *nächster* und `$Toolbar_4` *zum Ende* sind `disabled` → **so erkennt man die letzte Seite**; der Filter überlebt das Blättern (`p.info` nennt ihn weiter, die Rubrik *Aktive/r Filter* steht noch da) |
| `advanced_form.html` | `POST` mit dem Zustand von `results_filtered_page2.html` + `$Button$0=pressed&source=$B&focus=$$GFBO_7` | Erweiterte Suche: 4 Suchzeilen `$Select$0/$2/$4/$6` (Index) × `$Autosuggest$0…$3` (Term) × `$Select$1/$3/$5` (UND/ODER/NICHT), Indexvokabular (*Titel*, *Person*, *ISBN, ISSN, ISMN*, *Schlagwort*, …), Absenden mit `$Button$6` |
| `results_isbn.html` | `POST` mit dem Zustand von `advanced_form.html` + `$Select$0=ISBN, ISSN, ISMN&$Autosuggest$0=9783257229530&$Select$1=UND&$Button$6=pressed` | Feldsuche funktioniert (4 Treffer). Und die **zweite Schreibweise der Kopfzeile**: `Treffer: 4 im "Bibliotheksbestand"` statt `Treffer: 71 in Bibliotheksbestand` — eine Zahlenregex, nie ein Textvergleich |
| `noaccess.html` | Absichtlich kaputt: derselbe `requestCount` zweimal gesendet (303 → `…/noaccess`) | Der Ausfallmodus des Hauses: **HTTP 200**, URL endet auf `/noaccess`, **kein `<title>`**, **kein `<form>`**, Text „Bitte schließen Sie diesen Reiter“ / „Please close this tab“. Nie als „keine Treffer“ durchgehen lassen |

## Detailseiten (je eine eigene Sitzung)

`GET https://www.voebb.de/aDISWeb/app/prod00?sp=SPROD00&sp=<SAK…>` mit `-L` und leerem
Cookie-Jar. **Ohne Cookie-Jar antwortet der Server 302 auf dieselbe URL** — es braucht also
den Cookie-Handshake, aber keinen Formularzustand und keine vorherige Suche.
Jede Antwort trägt eine eigene neue Sitzungskennung und `requestCount=1`.

| Datei | Satz | Was sie belegt |
| --- | --- | --- |
| `detail_available.html` | `SAK13776205` | Der Normalfall: bibliografische Tabelle `table.gi` (`<th scope="row">Label</th><td>Wert</td>`), Exemplartabelle `table#resptable-1` mit **5** Spalten (Bibliothek, Standort, Signatur, Bestellmöglichkeit, Verfügbarkeit), Status `<span class="available">Verfügbar</span>`, Bestellmöglichkeit `Außenmagazin - Außenmagazin, bestellbar` |
| `detail_on_loan.html` | `SAK00177143` (der Roman selbst) | 46 Exemplarzeilen über das ganze Netz, und die Falle: dieselbe Tabelle hat hier nur **4** Spalten — **keine Signatur-Spalte**, die Signatur steckt hinter dem Bereich in *Standort*. Statusvokabular `Verfügbar` (38), `Ausgeliehen` (5), `Verloren` (1), `Nicht im Regal` (1) mit den Klassen `available`/`notavailable`. Kein Rückgabedatum |
| `detail_reference.html` | `SAK15912360` | Präsenzbestand: Verfügbarkeit sagt `Verfügbar`, die Nichtausleihbarkeit steht in **Bestellmöglichkeit**: `nicht entleihbar (Freihand) - Präsenzbestand`. Daneben eine zweite Zeile derselben Bibliothek als Magazinausleihe |
| `detail_online.html` | `SAK16112988` | E-Ressource/Onleihe: **`table#resptable-1` fehlt vollständig**; statt dessen eine `table.gi`-Zeile `Link zur Onleihe` mit Onleihe-URL und Leihstand im Linktext (`(Das Medium ist ausgeliehen / Vormerkung möglich)`) |
| `detail_multivolume.html` | `SAK13927817` | Mehrbändiges Werk: `Medienart` = `[Mehrteiliges Werk]`, `table#resptable-1` ist **vorhanden, aber leer** — `<tbody>` ohne Zeilen *und ohne `<thead>`*. Der dritte Fall neben „gefüllt“ und „fehlt“; die Bände sind eigene Sätze |

### Nachtrag 2026-09-06 (Phase 5.3/5.4, beim ersten Live-Lauf der Engine gefunden)

Zwei Fälle, die die erste Messrunde nicht getroffen hat. Beide sind mit denselben Rezepten
gezogen (`GET …/prod00?sp=SPROD00&sp=<SAK…>`, `-L`, Cookie-Jar, ehrlicher UA) und mit
`slim.py` eingedampft.

| Datei | Satz | Was sie belegt |
| --- | --- | --- |
| `detail_overdrive.html` | `SAK34672596` | **Die Onleihe ist nicht die einzige Ausleihplattform.** Dieselbe Bauart wie `detail_online`, aber die Zeile heißt `Link zu Overdrive` statt `Link zur Onleihe`, und daneben steht eine zusätzliche `URL`-Zeile. Ein Parser, der auf „Onleihe“ prüft, hält den Satz für eine kaputte Seite und bricht mit `resptable-1 matched nothing` ab — genau so ist der erste Live-Lauf gescheitert. Erkannt wird deshalb das Präfix `Link zu`, nie der Name der Plattform |
| `detail_unknown.html` | `SAK00000000` | **Eine unbekannte Satznummer beantwortet voebb.de mit der Suchstartseite**: HTTP 200, gültiges `Form0`, **kein `table.gi`**, **kein `div#R03`** (das hat jede echte Satzseite), dafür `div#R04`/`div#R05` wie `start.html`. Das ist der einzige Weg, „diesen Satz gibt es nicht“ (Exit 1) von „die Seite hat sich geändert“ (Exit 6) zu unterscheiden — deshalb werden **zwei** positive Merkmale geprüft und nicht nur das fehlende `table.gi` |

### Nachtrag 2026-09-08 (Runde 2, §3.6) — eine **abgeleitete** Datei

`detail_online_available.html` ist die einzige Datei hier, die **nicht** vom Server
stammt. Sie ist aus `detail_online.html` abgeleitet: eine einzige Zeile ist geändert, der
Linktext der `Link zur Onleihe`-Zeile, und sonst kein Byte (`diff` zeigt genau eine Zeile).

| Datei | Herkunft | Was sie belegt |
| --- | --- | --- |
| `detail_online_available.html` | **abgeleitet** aus `detail_online.html`: `(Das Medium ist ausgeliehen / Vormerkung möglich)` → `(Das Medium ist verfügbar / Ausleihe keine Vormerkung möglich)` | Der Leihstand einer E-Ressource im Klartext des Ausleihlinks, **positiver** Fall. `detail_online.html` deckt nur „ausgeliehen“ ab, `detail_overdrive.html` den Fall ganz **ohne** Klammer |

Warum abgeleitet und nicht gemessen: die drei Wortlaute sind in der Testrunde vom
Livesystem wörtlich mitgeschrieben worden (`plan/feedback_round_2.md` §3.6) —

```
Zugang zum Titel erhalten Sie hier. (Das Medium ist verfügbar / Ausleihe keine Vormerkung möglich)
Zugang zum Titel erhalten Sie hier. (Das Medium ist ausgeliehen / Vormerkung möglich)
Zugang zum Titel erhalten Sie hier.                      ← Overdrive, ganz ohne Klammer
```

— aber der Satz, der beim Abruf „verfügbar“ sagte, ist wenige Minuten später wieder
ausgeliehen. Ein zweiter Abruf hätte also nicht garantiert den fehlenden Fall geliefert.
Die Struktur ist deshalb **nicht erfunden**, sondern die gemessene: geändert ist nur der
Statuswortlaut innerhalb desselben `<a>`, und zwar der wörtlich mitgeschriebene.

### Nachtrag 2026-09-08 (Runde 2, §1.2) — die zweite **abgeleitete** Datei

`results_without_agb.html` ist die zweite Datei, die nicht vom Server stammt. Sie ist aus
`results.html` abgeleitet, indem **ein ganzes Element** entfernt wurde — dasselbe Rezept
wie `slim.py`, nur mit einer anderen Begründung: kein Zeichen des übrigen Markups ist
umgeschrieben.

| Datei | Herkunft | Was sie belegt |
| --- | --- | --- |
| `results_without_agb.html` | **abgeleitet** aus `results.html`: das eine `<li class="cbtree_leaf_li">` mit `sub-PTL1_tree_1_90` / *ZLB: Amerika-Gedenkbibliothek (AGB)* ist gelöscht (378 Byte), sonst nichts | Die Facette **führt die gesuchte Zweigstelle nicht, während die Suche netzweit Treffer hat** (`Treffer: 71` steht unverändert da). Das ist der Fall, in dem `--at` wirklich die Ursache des leeren Ergebnisses ist — der Gegenfall zu `results_empty.html`, wo die Suche schon netzweit null hatte und die Facette deshalb gar nicht existiert |

Warum abgeleitet und nicht gemessen: die Datei müsste eine Suche belegen, die 71 Treffer
im Netz und **keinen** in der AGB hat. So eine Anfrage ist gegen das Livesystem nur mit
Glück zu finden und morgen eine andere; das strukturelle Merkmal — die Rubrik *Bibliothek*
ohne den gesuchten Namen — ist dagegen genau das gemessene Dokument minus einen Knoten.
Die zweite ZLB-Zweigstelle (`_91`, BStB), die verschobenen Baumnummern und der
vollständige Rest des Baums bleiben unangetastet, ebenso `div#R06 p.info`, das Formular
und die Blätter-Toolbar. Reproduzierbar mit:

```python
s = open("results.html").read()
i = s.find('<li class="cbtree_leaf_li"><input type="checkbox" id="sub-PTL1_tree_1_90"')
open("results_without_agb.html", "w").write(s[:i] + s[s.find("</li>", i) + 5:])
```

Dazu eine Falle ohne eigene Fixture, belegt an `advanced_form.html`: die Seite trägt
**zwei** Schaltflächen mit der Beschriftung *Suchen* — die des Kopfzeilen-Suchschlitzes
(`$Button`, `$$GFBO_1`, zuerst im Dokument) und die des Formulars selbst (`$Button$6`,
`$$GFBO_4`). Wer die erste drückt, schickt eine leere Freitextsuche ab und wirft die
ausgefüllten Zeilen weg. Die Engine nimmt darum bei der erweiterten Suche die **letzte**
Schaltfläche dieser Beschriftung.

## Was gekürzt wurde

Zusammen waren es 1,1 MB, davon der Löwenanteil Rahmenwerk. `slim.py` entfernt daraus
**ganze Elemente**, byte-genau und ohne den Rest anzufassen:

| Weg | Warum |
| --- | --- |
| jedes `<script>`, jedes `<link>`, jedes `<meta>` außer `charset` | kein Selektor liest sie; das JS ist in `plan/voebb.md` § *Absendeprotokoll* ausgewertet und dort zitiert |
| `header#header`, `footer#footer`, `nav#sprunglinks`, `div#adis-timeout-counter` | Rahmen |
| `div.accordion` **ohne Formularfeld** (die Hilfetexte) | Prosa. Die Akkordeons *mit* Feldern bleiben: `div#R05` trägt die Sortierknöpfe `$Button$4…$8`, `div#R11` den Facettenbaum |
| die Bilderstrecke `ul.splide__list` der Startseite | 10 KB Werbung |
| je Trefferzeile `div.rList_cover`, `.rList_check`, `.rList_button`, `.rList_www`, `div.rList_panel` | Knöpfe für angemeldete Nutzer; `rList_num/_availability/_titel/_medium/_name/_jahr/_signatur` bleiben vollständig |
| die `<option>` ab der sechsten je `<select>` | 85 Hausnamen im Suchbereich, 53 Medienarten … Ausgenommen und **vollständig**: die Indexliste `SUCH01_*` und die Verknüpfung `LOP01_*` der erweiterten Suche, die genau das belegen |
| in `results_filtered.html` und `results_filtered_page2.html` die Trefferzeilen bis auf die ersten zwei, die letzte und je eine je Medienart/Ampel-Paar | die Zeilen sind dort nicht die Beweislast — die vollständige Seite mit **allen 22** Zeilen ist `results.html`. Was bleibt, ist das gemessene Vokabular: `Band`, `Buch`, `CD`, `DVD`, `MP3`, `Blu-ray Disc`, `E-Book`, `E-Audio`, `E-Ressource`, `Medienkombination` × `Verfügbar`, `Zurzeit nicht verfügbar`, `siehe Vollanzeige` |
| in `results_filtered*.html` und `results_isbn.html` die Facettenrubriken außer `Bibliothek` | der **vollständige** Baum mit allen sieben Rubriken steht in `results.html`. Die Indexverschiebung bleibt trotzdem lesbar: die Nummern sind Text im Dokument und ändern sich durch das Entfernen nicht |

Nicht angefasst: `noaccess.html` (die Datei ist der Beweis, dass dort weder `<title>` noch
`<form>` steht — an ihr darf nichts fehlen).

**Nie weg:** das `<form>` samt aller 9 versteckten Felder, `div#R06 p.info`, die
`Bibliothek`-Rubrik des Baums, die Blätter-Toolbars mit ihren `disabled`-Zuständen und
`table#resptable-1` mitsamt `<thead>`.

## Größen

| Datei | roh | jetzt |
| --- | ---: | ---: |
| `start.html` | 53 KiB | 12 KiB |
| `results.html` | 171 KiB | 96 KiB |
| `results_without_agb.html` | — (abgeleitet) | 96 KiB |
| `results_filtered.html` | 169 KiB | 58 KiB |
| `results_filtered_page2.html` | 146 KiB | 57 KiB |
| `results_isbn.html` | 92 KiB | 35 KiB |
| `advanced_form.html` | 111 KiB | 27 KiB |
| `noaccess.html` | 8 KiB | 8 KiB |
| `detail_*.html` (5) | 314 KiB | 91 KiB |
| **gesamt** | **1,1 MB** | **384 KiB** |
