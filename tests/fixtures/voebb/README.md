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
| `results_filtered.html` | 169 KiB | 58 KiB |
| `results_filtered_page2.html` | 146 KiB | 57 KiB |
| `results_isbn.html` | 92 KiB | 35 KiB |
| `advanced_form.html` | 111 KiB | 27 KiB |
| `noaccess.html` | 8 KiB | 8 KiB |
| `detail_*.html` (5) | 314 KiB | 91 KiB |
| **gesamt** | **1,1 MB** | **384 KiB** |
