# Condition data

`make srd` (`tools/fetch-srd.sh`) puts the SRD's conditions here (Blinded,
Prone and the rest, fifteen for each edition), in edda's
condition format (see the README's "Spells and conditions" section);
compose's seed job loads every `*.json` here into the monster service's
shared library. They aren't part of the repository (`.gitignore` keeps them
out); your own files of named effects can go here too.

| File | Edition |
| --- | --- |
| `srd-5.1.json` | 2014 rules |
| `srd-5.2.json` | 2024 rules |

They're converted from [5e-database](https://github.com/5e-bits/5e-database)'s
`5e-SRD-Conditions.json` files like the monsters (see
`data/monsters/README.md`); the conversion changes only the layout, not the
rules text.

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
