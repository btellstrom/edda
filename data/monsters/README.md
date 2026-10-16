# Monster data

`make srd` (`tools/fetch-srd-monsters.sh`) puts the SRD monsters here, in
campaign-manager's monster format (see the README's "Monsters" section);
compose's seed job loads every `*.json` here into the monster service's
shared library. They aren't part of the repository (`.gitignore` keeps them
out); your own monster files can go here too.

| File | Monsters | Edition |
| --- | --- | --- |
| `srd-5.1.json` | 334 | 2014 rules |
| `srd-5.2.json` | 341 | 2024 rules |

Wizards of the Coast publishes the SRD only as PDFs. The script downloads
the structured `5e-SRD-Monsters.json` files of
[5e-database](https://github.com/5e-bits/5e-database) (from `src/2014/en`
and `src/2024/en`, at a pinned commit, checked against known checksums)
and converts them with `tools/convert-5e-database.py`. The conversion
changes only the layout (field names, how speed, senses and usage are
written), not the rules text.

## Attribution

Whoever shares these files (or anything made from them) must keep this
attribution with them.

This work includes material taken from the System Reference Document 5.1
("SRD 5.1") by Wizards of the Coast LLC and available at
https://dnd.wizards.com/resources/systems-reference-document. The SRD 5.1 is
licensed under the Creative Commons Attribution 4.0 International License
available at https://creativecommons.org/licenses/by/4.0/legalcode.

This work includes material from the System Reference Document 5.2
("SRD 5.2") by Wizards of the Coast LLC, available at
https://www.dndbeyond.com/srd. The SRD 5.2 is licensed under the Creative
Commons Attribution 4.0 International License, available at
https://creativecommons.org/licenses/by/4.0/legalcode.
