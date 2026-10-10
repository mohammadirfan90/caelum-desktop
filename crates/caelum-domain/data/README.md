# caelum-domain data

| File | What | Source | Updated by |
|---|---|---|---|
| `Leap_Second.dat` | TAI−UTC table (R-OPS-05) | IERS Bulletin C 72, July 2026, <https://hpiers.obspm.fr/iers/bul/bulc/Leap_Second.dat>; expires 28 June 2027; SHA-256 `6cb6f5d4b819f2e568e25db4b0b26d89dedf031fdffb18bc94d40f4e94e268d7` | Signed configuration in operation; a new file per IERS bulletin in development |
| `loinc-subset.csv` | The LOINC codes CAELUM uses, verified (CR-003 §7) | LOINC 2.82 via the HL7 terminology server `tx.fhir.org` (`CodeSystem/$lookup`), 2026-10-10. When the LOINC table is placed in `tools/offline/loinc/`, the codes are re-checked against it | Code review; every catalog code must appear here (test) |

## LOINC notice
This material contains content from LOINC (<https://loinc.org>). LOINC is copyright © 1995-2026,
Regenstrief Institute, Inc. and the Logical Observation Identifiers Names and Codes (LOINC)
Committee and is available at no cost under the license at <https://loinc.org/license>. LOINC® is a
registered United States trademark of Regenstrief Institute, Inc.
