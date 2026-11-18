//! CVSS v4.0 base/threat/environmental score of a vector string.
//!
//! CVSS v4.0 has no closed formula: a vector is reduced to one of 270
//! MacroVectors (six "equivalence class" digits), each with an expert-assigned
//! score, and the final score interpolates within the MacroVector by the
//! vector's severity distance from the MacroVector's most severe vectors.
//!
//! This is a port of the FIRST reference calculator (`cvss40.js` from
//! <https://github.com/RedHatProductSecurity/cvss-v4-calculator>, which
//! <https://www.first.org/cvss/calculator/4.0> uses). It follows that code
//! step for step, including its floating-point arithmetic and rounding, so the
//! scores match the reference exactly; `tests` checks the published examples.
//!
//! The lookup tables and the algorithm are used under the reference
//! implementation's licence:
//!
//! > Copyright (c) 2023 FIRST.ORG, Inc., Red Hat, and contributors
//! >
//! > Redistribution and use in source and binary forms, with or without
//! > modification, are permitted provided that the following conditions are met:
//! >
//! > 1. Redistributions of source code must retain the above copyright notice,
//! >    this list of conditions and the following disclaimer.
//! >
//! > 2. Redistributions in binary form must reproduce the above copyright
//! >    notice, this list of conditions and the following disclaimer in the
//! >    documentation and/or other materials provided with the distribution.
//! >
//! > THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
//! > AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
//! > IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
//! > ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE
//! > LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR
//! > CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF
//! > SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS
//! > INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN
//! > CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE)
//! > ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE
//! > POSSIBILITY OF SUCH DAMAGE.

use std::collections::HashMap;

/// Every metric in specification order, with its allowed values (the first
/// is the default for an omitted non-base metric).
const METRICS: [(&str, &[&str]); 32] = [
    // Base (mandatory)
    ("AV", &["N", "A", "L", "P"]),
    ("AC", &["L", "H"]),
    ("AT", &["N", "P"]),
    ("PR", &["N", "L", "H"]),
    ("UI", &["N", "P", "A"]),
    ("VC", &["N", "L", "H"]),
    ("VI", &["N", "L", "H"]),
    ("VA", &["N", "L", "H"]),
    ("SC", &["N", "L", "H"]),
    ("SI", &["N", "L", "H"]),
    ("SA", &["N", "L", "H"]),
    // Threat
    ("E", &["X", "A", "P", "U"]),
    // Environmental
    ("CR", &["X", "H", "M", "L"]),
    ("IR", &["X", "H", "M", "L"]),
    ("AR", &["X", "H", "M", "L"]),
    ("MAV", &["X", "N", "A", "L", "P"]),
    ("MAC", &["X", "L", "H"]),
    ("MAT", &["X", "N", "P"]),
    ("MPR", &["X", "N", "L", "H"]),
    ("MUI", &["X", "N", "P", "A"]),
    ("MVC", &["X", "H", "L", "N"]),
    ("MVI", &["X", "H", "L", "N"]),
    ("MVA", &["X", "H", "L", "N"]),
    ("MSC", &["X", "H", "L", "N"]),
    ("MSI", &["X", "S", "H", "L", "N"]),
    ("MSA", &["X", "S", "H", "L", "N"]),
    // Supplemental (not scored)
    ("S", &["X", "N", "P"]),
    ("AU", &["X", "N", "Y"]),
    ("R", &["X", "A", "U", "I"]),
    ("V", &["X", "D", "C"]),
    ("RE", &["X", "L", "M", "H"]),
    ("U", &["X", "Clear", "Green", "Amber", "Red"]),
];

/// Number of mandatory (base) metrics at the start of [`METRICS`].
const BASE_METRICS: usize = 11;

/// MacroVector (EQ1..EQ6 digits) → score of its most severe vector.
const LOOKUP: [(&str, f64); 270] = [
    ("000000", 10.0),
    ("000001", 9.9),
    ("000010", 9.8),
    ("000011", 9.5),
    ("000020", 9.5),
    ("000021", 9.2),
    ("000100", 10.0),
    ("000101", 9.6),
    ("000110", 9.3),
    ("000111", 8.7),
    ("000120", 9.1),
    ("000121", 8.1),
    ("000200", 9.3),
    ("000201", 9.0),
    ("000210", 8.9),
    ("000211", 8.0),
    ("000220", 8.1),
    ("000221", 6.8),
    ("001000", 9.8),
    ("001001", 9.5),
    ("001010", 9.5),
    ("001011", 9.2),
    ("001020", 9.0),
    ("001021", 8.4),
    ("001100", 9.3),
    ("001101", 9.2),
    ("001110", 8.9),
    ("001111", 8.1),
    ("001120", 8.1),
    ("001121", 6.5),
    ("001200", 8.8),
    ("001201", 8.0),
    ("001210", 7.8),
    ("001211", 7.0),
    ("001220", 6.9),
    ("001221", 4.8),
    ("002001", 9.2),
    ("002011", 8.2),
    ("002021", 7.2),
    ("002101", 7.9),
    ("002111", 6.9),
    ("002121", 5.0),
    ("002201", 6.9),
    ("002211", 5.5),
    ("002221", 2.7),
    ("010000", 9.9),
    ("010001", 9.7),
    ("010010", 9.5),
    ("010011", 9.2),
    ("010020", 9.2),
    ("010021", 8.5),
    ("010100", 9.5),
    ("010101", 9.1),
    ("010110", 9.0),
    ("010111", 8.3),
    ("010120", 8.4),
    ("010121", 7.1),
    ("010200", 9.2),
    ("010201", 8.1),
    ("010210", 8.2),
    ("010211", 7.1),
    ("010220", 7.2),
    ("010221", 5.3),
    ("011000", 9.5),
    ("011001", 9.3),
    ("011010", 9.2),
    ("011011", 8.5),
    ("011020", 8.5),
    ("011021", 7.3),
    ("011100", 9.2),
    ("011101", 8.2),
    ("011110", 8.0),
    ("011111", 7.2),
    ("011120", 7.0),
    ("011121", 5.9),
    ("011200", 8.4),
    ("011201", 7.0),
    ("011210", 7.1),
    ("011211", 5.2),
    ("011220", 5.0),
    ("011221", 3.0),
    ("012001", 8.6),
    ("012011", 7.5),
    ("012021", 5.2),
    ("012101", 7.1),
    ("012111", 5.2),
    ("012121", 2.9),
    ("012201", 6.3),
    ("012211", 2.9),
    ("012221", 1.7),
    ("100000", 9.8),
    ("100001", 9.5),
    ("100010", 9.4),
    ("100011", 8.7),
    ("100020", 9.1),
    ("100021", 8.1),
    ("100100", 9.4),
    ("100101", 8.9),
    ("100110", 8.6),
    ("100111", 7.4),
    ("100120", 7.7),
    ("100121", 6.4),
    ("100200", 8.7),
    ("100201", 7.5),
    ("100210", 7.4),
    ("100211", 6.3),
    ("100220", 6.3),
    ("100221", 4.9),
    ("101000", 9.4),
    ("101001", 8.9),
    ("101010", 8.8),
    ("101011", 7.7),
    ("101020", 7.6),
    ("101021", 6.7),
    ("101100", 8.6),
    ("101101", 7.6),
    ("101110", 7.4),
    ("101111", 5.8),
    ("101120", 5.9),
    ("101121", 5.0),
    ("101200", 7.2),
    ("101201", 5.7),
    ("101210", 5.7),
    ("101211", 5.2),
    ("101220", 5.2),
    ("101221", 2.5),
    ("102001", 8.3),
    ("102011", 7.0),
    ("102021", 5.4),
    ("102101", 6.5),
    ("102111", 5.8),
    ("102121", 2.6),
    ("102201", 5.3),
    ("102211", 2.1),
    ("102221", 1.3),
    ("110000", 9.5),
    ("110001", 9.0),
    ("110010", 8.8),
    ("110011", 7.6),
    ("110020", 7.6),
    ("110021", 7.0),
    ("110100", 9.0),
    ("110101", 7.7),
    ("110110", 7.5),
    ("110111", 6.2),
    ("110120", 6.1),
    ("110121", 5.3),
    ("110200", 7.7),
    ("110201", 6.6),
    ("110210", 6.8),
    ("110211", 5.9),
    ("110220", 5.2),
    ("110221", 3.0),
    ("111000", 8.9),
    ("111001", 7.8),
    ("111010", 7.6),
    ("111011", 6.7),
    ("111020", 6.2),
    ("111021", 5.8),
    ("111100", 7.4),
    ("111101", 5.9),
    ("111110", 5.7),
    ("111111", 5.7),
    ("111120", 4.7),
    ("111121", 2.3),
    ("111200", 6.1),
    ("111201", 5.2),
    ("111210", 5.7),
    ("111211", 2.9),
    ("111220", 2.4),
    ("111221", 1.6),
    ("112001", 7.1),
    ("112011", 5.9),
    ("112021", 3.0),
    ("112101", 5.8),
    ("112111", 2.6),
    ("112121", 1.5),
    ("112201", 2.3),
    ("112211", 1.3),
    ("112221", 0.6),
    ("200000", 9.3),
    ("200001", 8.7),
    ("200010", 8.6),
    ("200011", 7.2),
    ("200020", 7.5),
    ("200021", 5.8),
    ("200100", 8.6),
    ("200101", 7.4),
    ("200110", 7.4),
    ("200111", 6.1),
    ("200120", 5.6),
    ("200121", 3.4),
    ("200200", 7.0),
    ("200201", 5.4),
    ("200210", 5.2),
    ("200211", 4.0),
    ("200220", 4.0),
    ("200221", 2.2),
    ("201000", 8.5),
    ("201001", 7.5),
    ("201010", 7.4),
    ("201011", 5.5),
    ("201020", 6.2),
    ("201021", 5.1),
    ("201100", 7.2),
    ("201101", 5.7),
    ("201110", 5.5),
    ("201111", 4.1),
    ("201120", 4.6),
    ("201121", 1.9),
    ("201200", 5.3),
    ("201201", 3.6),
    ("201210", 3.4),
    ("201211", 1.9),
    ("201220", 1.9),
    ("201221", 0.8),
    ("202001", 6.4),
    ("202011", 5.1),
    ("202021", 2.0),
    ("202101", 4.7),
    ("202111", 2.1),
    ("202121", 1.1),
    ("202201", 2.4),
    ("202211", 0.9),
    ("202221", 0.4),
    ("210000", 8.8),
    ("210001", 7.5),
    ("210010", 7.3),
    ("210011", 5.3),
    ("210020", 6.0),
    ("210021", 5.0),
    ("210100", 7.3),
    ("210101", 5.5),
    ("210110", 5.9),
    ("210111", 4.0),
    ("210120", 4.1),
    ("210121", 2.0),
    ("210200", 5.4),
    ("210201", 4.3),
    ("210210", 4.5),
    ("210211", 2.2),
    ("210220", 2.0),
    ("210221", 1.1),
    ("211000", 7.5),
    ("211001", 5.5),
    ("211010", 5.8),
    ("211011", 4.5),
    ("211020", 4.0),
    ("211021", 2.1),
    ("211100", 6.1),
    ("211101", 5.1),
    ("211110", 4.8),
    ("211111", 1.8),
    ("211120", 2.0),
    ("211121", 0.9),
    ("211200", 4.6),
    ("211201", 1.8),
    ("211210", 1.7),
    ("211211", 0.7),
    ("211220", 0.8),
    ("211221", 0.2),
    ("212001", 5.3),
    ("212011", 2.4),
    ("212021", 1.4),
    ("212101", 2.4),
    ("212111", 1.2),
    ("212121", 0.5),
    ("212201", 1.0),
    ("212211", 0.3),
    ("212221", 0.1),
];

/// Severity level of each scored metric value, used for the distance from a
/// MacroVector's most severe vector (`METRIC_LEVELS`).
const METRIC_LEVELS: [(&str, &[(&str, f64)]); 15] = [
    ("AV", &[("N", 0.0), ("A", 0.1), ("L", 0.2), ("P", 0.3)]),
    ("PR", &[("N", 0.0), ("L", 0.1), ("H", 0.2)]),
    ("UI", &[("N", 0.0), ("P", 0.1), ("A", 0.2)]),
    ("AC", &[("L", 0.0), ("H", 0.1)]),
    ("AT", &[("N", 0.0), ("P", 0.1)]),
    ("VC", &[("H", 0.0), ("L", 0.1), ("N", 0.2)]),
    ("VI", &[("H", 0.0), ("L", 0.1), ("N", 0.2)]),
    ("VA", &[("H", 0.0), ("L", 0.1), ("N", 0.2)]),
    ("SC", &[("H", 0.1), ("L", 0.2), ("N", 0.3)]),
    ("SI", &[("S", 0.0), ("H", 0.1), ("L", 0.2), ("N", 0.3)]),
    ("SA", &[("S", 0.0), ("H", 0.1), ("L", 0.2), ("N", 0.3)]),
    ("CR", &[("H", 0.0), ("M", 0.1), ("L", 0.2)]),
    ("IR", &[("H", 0.0), ("M", 0.1), ("L", 0.2)]),
    ("AR", &[("H", 0.0), ("M", 0.1), ("L", 0.2)]),
    ("E", &[("U", 0.2), ("P", 0.1), ("A", 0.0)]),
];

/// The most severe vectors of each EQ level (`MAX_COMPOSED`).
fn max_composed(eq: usize, level: u8) -> &'static [&'static str] {
    match (eq, level) {
        (1, 0) => &["AV:N/PR:N/UI:N/"],
        (1, 1) => &["AV:A/PR:N/UI:N/", "AV:N/PR:L/UI:N/", "AV:N/PR:N/UI:P/"],
        (1, 2) => &["AV:P/PR:N/UI:N/", "AV:A/PR:L/UI:P/"],
        (2, 0) => &["AC:L/AT:N/"],
        (2, 1) => &["AC:H/AT:N/", "AC:L/AT:P/"],
        (4, 0) => &["SC:H/SI:S/SA:S/"],
        (4, 1) => &["SC:H/SI:H/SA:H/"],
        (4, 2) => &["SC:L/SI:L/SA:L/"],
        (5, 0) => &["E:A/"],
        (5, 1) => &["E:P/"],
        (5, 2) => &["E:U/"],
        _ => &[],
    }
}

/// The most severe vectors of an EQ3+EQ6 level (`MAX_COMPOSED.eq3`).
fn max_composed_eq3(eq3: u8, eq6: u8) -> &'static [&'static str] {
    match (eq3, eq6) {
        (0, 0) => &["VC:H/VI:H/VA:H/CR:H/IR:H/AR:H/"],
        (0, 1) => &[
            "VC:H/VI:H/VA:L/CR:M/IR:M/AR:H/",
            "VC:H/VI:H/VA:H/CR:M/IR:M/AR:M/",
        ],
        (1, 0) => &[
            "VC:L/VI:H/VA:H/CR:H/IR:H/AR:H/",
            "VC:H/VI:L/VA:H/CR:H/IR:H/AR:H/",
        ],
        (1, 1) => &[
            "VC:L/VI:H/VA:L/CR:H/IR:M/AR:H/",
            "VC:L/VI:H/VA:H/CR:H/IR:M/AR:M/",
            "VC:H/VI:L/VA:H/CR:M/IR:H/AR:M/",
            "VC:H/VI:L/VA:L/CR:M/IR:H/AR:H/",
            "VC:L/VI:L/VA:H/CR:H/IR:H/AR:M/",
        ],
        (2, 1) => &["VC:L/VI:L/VA:L/CR:H/IR:H/AR:H/"],
        _ => &[],
    }
}

/// Depth of each EQ level, in metric-level steps (`MAX_SEVERITY`).
fn max_severity(eq: usize, level: u8) -> Option<f64> {
    let depths: &[u8] = match eq {
        1 => &[1, 4, 5],
        2 => &[1, 2],
        4 => &[6, 5, 4],
        _ => return None,
    };
    depths.get(usize::from(level)).map(|d| f64::from(*d))
}

/// Depth of an EQ3+EQ6 level (`MAX_SEVERITY.eq3eq6`).
fn max_severity_eq3eq6(eq3: u8, eq6: u8) -> Option<f64> {
    let depth = match (eq3, eq6) {
        (0, 0) => 7,
        (0, 1) => 6,
        (1, 0 | 1) => 8,
        (2, 1) => 10,
        _ => return None,
    };
    Some(f64::from(depth))
}

fn lookup(macro_vector: &[u8; 6]) -> Option<f64> {
    let key: String = macro_vector.iter().map(|d| char::from(b'0' + d)).collect();
    LOOKUP
        .binary_search_by(|(k, _)| (*k).cmp(key.as_str()))
        .ok()
        .map(|i| LOOKUP[i].1)
}

fn level(metric: &str, value: &str) -> Option<f64> {
    METRIC_LEVELS
        .iter()
        .find(|(m, _)| *m == metric)?
        .1
        .iter()
        .find(|(v, _)| *v == value)
        .map(|(_, l)| *l)
}

/// A parsed, validated vector.
struct Vector<'a> {
    metrics: HashMap<&'static str, &'a str>,
}

impl<'a> Vector<'a> {
    /// Parse `CVSS:4.0/AV:…/…`. Every base metric is required, and every
    /// metric must be known, allowed and given once. Unlike the reference's
    /// validator, metrics may come in any order (as other implementations
    /// accept), which does not change the score.
    fn parse(vector: &'a str) -> Option<Self> {
        let mut parts = vector.trim().split('/');
        if parts.next()? != "CVSS:4.0" {
            return None;
        }
        let mut metrics: HashMap<&'static str, &'a str> = HashMap::new();
        for part in parts {
            let (key, value) = part.split_once(':')?;
            let (metric, allowed) = METRICS.iter().find(|(m, _)| *m == key)?;
            if !allowed.contains(&value) || metrics.insert(metric, value).is_some() {
                return None;
            }
        }
        if !METRICS[..BASE_METRICS]
            .iter()
            .all(|(m, _)| metrics.contains_key(m))
        {
            return None;
        }
        for (metric, values) in &METRICS[BASE_METRICS..] {
            metrics.entry(metric).or_insert(values[0]);
        }
        Some(Self { metrics })
    }

    /// The value scoring uses for `metric`: `E`/`CR`/`IR`/`AR` default to the
    /// worst case, and a modified (`M…`) environmental metric overrides its
    /// base metric (`getEffectiveMetricValue`).
    fn effective(&self, metric: &str) -> &str {
        let value = self.metrics.get(metric).copied().unwrap_or("X");
        if value == "X" {
            match metric {
                "E" => return "A",
                "CR" | "IR" | "AR" => return "H",
                _ => {},
            }
        }
        if let Some(modified) = self.metrics.get(format!("M{metric}").as_str()) {
            if *modified != "X" {
                return modified;
            }
        }
        value
    }

    /// EQ1..EQ6 (`equivalentClasses`).
    fn macro_vector(&self) -> [u8; 6] {
        let e = |m| self.effective(m);
        let (av, pr, ui) = (e("AV"), e("PR"), e("UI"));
        let eq1 = if av == "N" && pr == "N" && ui == "N" {
            0
        } else if (av == "N" || pr == "N" || ui == "N") && av != "P" {
            1
        } else {
            2
        };
        let eq2 = u8::from(!(e("AC") == "L" && e("AT") == "N"));
        let (vc, vi, va) = (e("VC"), e("VI"), e("VA"));
        let eq3 = if vc == "H" && vi == "H" {
            0
        } else if vc == "H" || vi == "H" || va == "H" {
            1
        } else {
            2
        };
        let eq4 = if e("MSI") == "S" || e("MSA") == "S" {
            0
        } else if e("SC") == "H" || e("SI") == "H" || e("SA") == "H" {
            1
        } else {
            2
        };
        let eq5 = match e("E") {
            "A" => 0,
            "P" => 1,
            _ => 2,
        };
        let eq6 = u8::from(
            !((e("CR") == "H" && vc == "H")
                || (e("IR") == "H" && vi == "H")
                || (e("AR") == "H" && va == "H")),
        );
        [eq1, eq2, eq3, eq4, eq5, eq6]
    }

    /// Per-metric distance from a most severe vector (`calculateSeverityDistances`).
    fn distances(&self, max_vector: &str) -> Option<HashMap<&'static str, f64>> {
        let max: HashMap<&str, &str> = max_vector
            .split('/')
            .filter_map(|p| p.split_once(':'))
            .collect();
        METRIC_LEVELS
            .iter()
            .map(|(metric, _)| {
                let current = level(metric, self.effective(metric))?;
                let highest = level(metric, max.get(metric)?)?;
                Some((*metric, current - highest))
            })
            .collect()
    }
}

/// Round half up to one decimal, with the reference's epsilon
/// (`roundToDecimalPlaces`).
fn round1(value: f64) -> f64 {
    ((value + 1e-6) * 10.0 + 0.5).floor() / 10.0
}

/// The CVSS v4.0 score (0.0–10.0) of a `CVSS:4.0/…` vector, or `None` when
/// the vector is not valid CVSS v4.0.
#[must_use]
pub fn score(vector: &str) -> Option<f64> {
    let v = Vector::parse(vector)?;

    // No impact on either system.
    if ["VC", "VI", "VA", "SC", "SI", "SA"]
        .iter()
        .all(|m| v.effective(m) == "N")
    {
        return Some(0.0);
    }

    let mv = v.macro_vector();
    let value = lookup(&mv)?;
    let [eq1, eq2, eq3, eq4, eq5, eq6] = mv;

    // 1a. Score of the next lower MacroVector per EQ (None: there is none).
    let lower = |i: usize, eq3eq6: Option<(u8, u8)>| {
        let mut next = mv;
        if let Some((d3, d6)) = eq3eq6 {
            next[2] += d3;
            next[5] += d6;
        } else {
            next[i] += 1;
        }
        lookup(&next)
    };
    let lower_eq1 = lower(0, None);
    let lower_eq2 = lower(1, None);
    let lower_eq3eq6 = match (eq3, eq6) {
        // 00 → 01 or 10: the higher of the two (NaN in the reference if
        // either is missing).
        (0, 0) => match (lower(0, Some((0, 1))), lower(0, Some((1, 0)))) {
            (Some(left), Some(right)) => Some(left.max(right)),
            _ => None,
        },
        // 01 → 11, 11 → 21
        (0 | 1, 1) => lower(0, Some((1, 0))),
        // 10 → 11
        (1, 0) => lower(0, Some((0, 1))),
        // 21 → 32 (does not exist)
        _ => lower(0, Some((1, 1))),
    };
    let lower_eq4 = lower(3, None);
    let lower_eq5 = lower(4, None);

    // 1b. Distance from the first most severe vector of the MacroVector that
    // is at least as severe as this one in every metric.
    let eq3_maxes = max_composed_eq3(eq3, eq6);
    let mut distances = None;
    'search: for a in max_composed(1, eq1) {
        for b in max_composed(2, eq2) {
            for c in eq3_maxes {
                for d in max_composed(4, eq4) {
                    for e in max_composed(5, eq5) {
                        let candidate = v.distances(&format!("{a}{b}{c}{d}{e}"))?;
                        let fits = candidate.values().all(|d| *d >= 0.0);
                        distances = Some(candidate);
                        if fits {
                            break 'search;
                        }
                    }
                }
            }
        }
    }
    let d = distances?;
    let current_eq1 = d["AV"] + d["PR"] + d["UI"];
    let current_eq2 = d["AC"] + d["AT"];
    let current_eq3eq6 = d["VC"] + d["VI"] + d["VA"] + d["CR"] + d["IR"] + d["AR"];
    let current_eq4 = d["SC"] + d["SI"] + d["SA"];

    // 1c/d. Proportional distance times the available score difference.
    const STEP: f64 = 0.1;
    let mut existing = 0_u32;
    let mut normalized = [0.0_f64; 5];
    let parts = [
        (lower_eq1, current_eq1, max_severity(1, eq1)),
        (lower_eq2, current_eq2, max_severity(2, eq2)),
        (lower_eq3eq6, current_eq3eq6, max_severity_eq3eq6(eq3, eq6)),
        (lower_eq4, current_eq4, max_severity(4, eq4)),
    ];
    for (slot, (lower, current, depth)) in normalized.iter_mut().zip(parts) {
        if let Some(lower) = lower {
            existing += 1;
            let percent = current / (depth? * STEP);
            *slot = (value - lower) * percent;
        }
    }
    // EQ5's proportion is always 0.
    if lower_eq5.is_some() {
        existing += 1;
    }

    // 2. Mean of the proportional distances.
    let mean = if existing == 0 {
        0.0
    } else {
        normalized.iter().sum::<f64>() / f64::from(existing)
    };

    // 3. MacroVector score minus the mean distance.
    Some(round1((value - mean).clamp(0.0, 10.0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sample of the reference test vectors (`tests/vectors_*4` of
    /// <https://github.com/RedHatProductSecurity/cvss>): base, threat,
    /// security requirements, supplemental, modified and random vectors. The
    /// port matched all 579,918 vectors in those files when it was written.
    #[test]
    fn matches_reference_scores() {
        for (vector, expected) in [
            (
                "CVSS:4.0/AV:N/AC:L/AT:N/PR:N/UI:N/VC:N/VI:N/VA:N/SC:N/SI:N/SA:N/E:A",
                0.0,
            ),
            (
                "CVSS:4.0/AV:P/AC:H/AT:P/PR:H/UI:A/VC:H/VI:H/VA:H/SC:H/SI:H/SA:H/E:A",
                7.3,
            ),
            (
                "CVSS:4.0/AV:N/AC:L/AT:N/PR:N/UI:N/VC:N/VI:N/VA:N/SC:N/SI:N/SA:N/E:P",
                0.0,
            ),
            (
                "CVSS:4.0/AV:P/AC:H/AT:P/PR:H/UI:A/VC:H/VI:H/VA:H/SC:H/SI:H/SA:H/E:P",
                5.9,
            ),
            (
                "CVSS:4.0/AV:N/AC:L/AT:N/PR:N/UI:N/VC:N/VI:N/VA:N/SC:N/SI:N/SA:N/E:U",
                0.0,
            ),
            (
                "CVSS:4.0/AV:P/AC:H/AT:P/PR:H/UI:A/VC:H/VI:H/VA:H/SC:H/SI:H/SA:H/E:U",
                4.1,
            ),
            (
                "CVSS:4.0/AV:L/AC:H/AT:N/PR:N/UI:P/VC:N/VI:L/VA:H/SC:H/SI:H/SA:H",
                7.1,
            ),
            (
                "CVSS:4.0/AV:A/AC:L/AT:N/PR:L/UI:P/VC:L/VI:H/VA:N/SC:N/SI:H/SA:N",
                6.7,
            ),
            (
                "CVSS:4.0/AV:P/AC:L/AT:N/PR:L/UI:P/VC:H/VI:L/VA:N/SC:L/SI:L/SA:L",
                5.2,
            ),
            (
                "CVSS:4.0/AV:N/AC:L/AT:P/PR:L/UI:N/VC:L/VI:L/VA:H/SC:L/SI:L/SA:H",
                7.2,
            ),
            (
                "CVSS:4.0/AV:N/AC:H/AT:N/PR:N/UI:N/VC:L/VI:N/VA:H/SC:H/SI:L/SA:H",
                9.1,
            ),
            (
                "CVSS:4.0/AV:N/AC:H/AT:N/PR:H/UI:N/VC:N/VI:N/VA:L/SC:L/SI:L/SA:H",
                5.1,
            ),
            (
                "CVSS:4.0/AV:L/AC:H/AT:P/PR:L/UI:P/VC:L/VI:L/VA:L/SC:L/SI:H/SA:H",
                2.3,
            ),
            (
                "CVSS:4.0/AV:N/AC:L/AT:P/PR:L/UI:A/VC:N/VI:N/VA:N/SC:L/SI:N/SA:H",
                4.5,
            ),
            (
                "CVSS:4.0/AV:P/AC:H/AT:P/PR:H/UI:N/VC:L/VI:N/VA:N/SC:H/SI:N/SA:N",
                1.8,
            ),
            (
                "CVSS:4.0/AV:A/AC:H/AT:N/PR:L/UI:P/VC:H/VI:L/VA:H/SC:N/SI:H/SA:L",
                5.8,
            ),
            (
                "CVSS:4.0/AV:N/AC:L/AT:P/PR:N/UI:N/VC:N/VI:L/VA:L/SC:H/SI:L/SA:L",
                7.0,
            ),
            (
                "CVSS:4.0/AV:N/AC:H/AT:N/PR:L/UI:P/VC:H/VI:N/VA:N/SC:N/SI:L/SA:L",
                5.9,
            ),
            (
                "CVSS:4.0/AV:P/AC:H/AT:P/PR:H/UI:A/VC:H/VI:H/VA:H/SC:H/SI:H/SA:H/CR:M/IR:M/AR:M",
                5.5,
            ),
            (
                "CVSS:4.0/AV:N/AC:L/AT:N/PR:N/UI:N/VC:N/VI:N/VA:N/SC:N/SI:N/SA:N/CR:M/IR:M/AR:M",
                0.0,
            ),
            (
                "CVSS:4.0/AV:N/AC:L/AT:N/PR:N/UI:N/VC:N/VI:N/VA:N/SC:N/SI:N/SA:N/CR:H/IR:H/AR:L",
                0.0,
            ),
            (
                "CVSS:4.0/AV:P/AC:H/AT:P/PR:H/UI:A/VC:H/VI:H/VA:H/SC:H/SI:H/SA:H/CR:H/IR:L/AR:M",
                7.1,
            ),
            (
                "CVSS:4.0/AV:N/AC:L/AT:N/PR:N/UI:N/VC:N/VI:N/VA:N/SC:N/SI:N/SA:N/S:N/AU:N/R:U/V:C/RE:H/U:Amber",
                0.0,
            ),
            (
                "CVSS:4.0/AV:N/AC:L/AT:N/PR:N/UI:N/VC:N/VI:N/VA:N/SC:N/SI:N/SA:N/S:P/AU:Y/R:I/V:C/RE:M/U:Amber",
                0.0,
            ),
            (
                "CVSS:4.0/AV:N/AC:L/AT:N/PR:N/UI:N/VC:N/VI:N/VA:N/SC:N/SI:N/SA:N/S:P/AU:Y/R:A/V:D/RE:L/U:Green",
                0.0,
            ),
            (
                "CVSS:4.0/AV:N/AC:L/AT:N/PR:N/UI:N/VC:H/VI:H/VA:H/SC:H/SI:H/SA:H",
                10.0,
            ),
            (
                "CVSS:4.0/AV:N/AC:L/AT:P/PR:N/UI:P/VC:L/VI:N/VA:L/SC:N/SI:N/SA:L",
                2.3,
            ),
            (
                "CVSS:4.0/AV:L/AC:H/AT:P/PR:N/UI:A/VC:N/VI:H/VA:H/SC:H/SI:H/SA:H",
                7.1,
            ),
            (
                "CVSS:4.0/AV:N/AC:H/AT:N/PR:L/UI:P/VC:H/VI:N/VA:H/SC:N/SI:N/SA:N",
                5.9,
            ),
            (
                "CVSS:4.0/AV:A/AC:L/AT:N/PR:L/UI:P/VC:H/VI:L/VA:H/SC:N/SI:H/SA:L",
                6.9,
            ),
            (
                "CVSS:4.0/AV:P/AC:L/AT:N/PR:L/UI:A/VC:L/VI:H/VA:L/SC:L/SI:H/SA:L",
                7.0,
            ),
            (
                "CVSS:4.0/AV:P/AC:L/AT:N/PR:L/UI:P/VC:N/VI:L/VA:H/SC:N/SI:L/SA:N",
                5.2,
            ),
            (
                "CVSS:4.0/AV:L/AC:H/AT:P/PR:L/UI:A/VC:N/VI:L/VA:L/SC:H/SI:L/SA:L",
                2.1,
            ),
            (
                "CVSS:4.0/AV:N/AC:L/AT:P/PR:N/UI:A/VC:H/VI:L/VA:H/SC:H/SI:N/SA:N",
                7.1,
            ),
            (
                "CVSS:4.0/AV:L/AC:L/AT:P/PR:L/UI:N/VC:N/VI:H/VA:H/SC:L/SI:H/SA:H/S:P/AU:Y/R:A/V:C/RE:H/U:Amber/MAV:A/MAC:L/MAT:N/MPR:L/MUI:P/MVC:H/MVI:L/MVA:N/MSC:H/MSI:H/MSA:L/CR:L/IR:L/AR:H/E:U",
                1.7,
            ),
            (
                "CVSS:4.0/AV:P/AC:H/AT:N/PR:L/UI:N/VC:H/VI:H/VA:L/SC:L/SI:N/SA:N/S:P/AU:N/R:I/V:C/RE:L/U:Amber/MAV:L/MAC:H/MAT:N/MPR:N/MUI:A/MVC:H/MVI:N/MVA:L/MSC:H/MSI:L/MSA:H/CR:L/IR:L/AR:M/E:P",
                4.3,
            ),
            (
                "CVSS:4.0/AV:L/AC:L/AT:N/PR:L/UI:P/VC:N/VI:H/VA:N/SC:L/SI:H/SA:L/S:P/AU:N/R:U/V:C/RE:L/U:Green/MAV:L/MAC:L/MAT:P/MPR:H/MUI:P/MVC:H/MVI:N/MVA:L/MSC:N/MSI:L/MSA:L/CR:H/IR:L/AR:H/E:U",
                0.5,
            ),
            (
                "CVSS:4.0/AV:L/AC:H/AT:N/PR:L/UI:P/VC:L/VI:N/VA:N/SC:L/SI:N/SA:N/S:N/AU:Y/R:I/V:D/RE:M/U:Red/MAV:N/MAC:H/MAT:N/MPR:H/MUI:N/MVC:H/MVI:H/MVA:N/MSC:L/MSI:N/MSA:H/CR:M/IR:M/AR:L/E:A",
                7.2,
            ),
            (
                "CVSS:4.0/AV:A/AC:L/AT:P/PR:N/UI:P/VC:L/VI:H/VA:H/SC:N/SI:H/SA:N/S:N/AU:Y/R:I/V:C/RE:M/U:Clear/MAV:P/MAC:L/MAT:N/MPR:L/MUI:P/MVC:H/MVI:L/MVA:H/MSC:N/MSI:L/MSA:S/CR:M/IR:H/AR:H/E:U",
                5.8,
            ),
            (
                "CVSS:4.0/AV:A/AC:L/AT:N/PR:H/UI:A/VC:L/VI:N/VA:L/SC:N/SI:L/SA:N/S:P/AU:Y/R:A/V:D/RE:M/U:Green/MAV:A/MAC:L/MAT:P/MPR:H/MUI:P/MVC:N/MVI:H/MVA:L/MSC:N/MSI:S/MSA:L/CR:L/IR:L/AR:M/E:U",
                1.5,
            ),
            (
                "CVSS:4.0/AV:P/AC:H/AT:N/PR:H/UI:A/VC:N/VI:N/VA:N/SC:H/SI:L/SA:L/S:P/AU:Y/R:A/V:D/RE:L/U:Amber/MAV:L/MAC:H/MAT:N/MPR:L/MUI:A/MVC:N/MVI:H/MVA:N/MSC:H/MSI:S/MSA:L/CR:H/IR:H/AR:H/E:A",
                7.1,
            ),
            (
                "CVSS:4.0/AV:P/AC:L/AT:N/PR:H/UI:P/VC:N/VI:N/VA:L/SC:N/SI:N/SA:H/S:P/AU:N/R:U/V:C/RE:H/U:Green/MAV:N/MAC:H/MAT:N/MPR:L/MUI:P/MVC:H/MVI:N/MVA:H/MSC:L/MSI:H/MSA:S/CR:M/IR:M/AR:M/E:P",
                6.4,
            ),
            (
                "CVSS:4.0/AV:N/AC:L/AT:N/PR:N/UI:N/VC:N/VI:N/VA:N/SC:N/SI:N/SA:N/MAV:A/MAC:H/MAT:N/MPR:L/MUI:P/MVC:L/MVI:N/MVA:H/MSC:L/MSI:N/MSA:N",
                4.3,
            ),
            (
                "CVSS:4.0/AV:P/AC:H/AT:P/PR:H/UI:A/VC:H/VI:H/VA:H/SC:H/SI:H/SA:H/MAV:L/MAC:L/MAT:P/MPR:L/MUI:N/MVC:N/MVI:L/MVA:H/MSC:H/MSI:S/MSA:L",
                8.5,
            ),
            (
                "CVSS:4.0/AV:P/AC:H/AT:P/PR:H/UI:A/VC:H/VI:H/VA:H/SC:H/SI:H/SA:H/MAV:N/MAC:H/MAT:P/MPR:N/MUI:A/MVC:H/MVI:L/MVA:L/MSC:N/MSI:H/MSA:N",
                7.0,
            ),
            (
                "CVSS:4.0/AV:P/AC:H/AT:P/PR:H/UI:A/VC:H/VI:H/VA:H/SC:H/SI:H/SA:H/MAV:P/MAC:L/MAT:N/MPR:N/MUI:P/MVC:L/MVI:H/MVA:H/MSC:N/MSI:L/MSA:H",
                6.9,
            ),
            (
                "CVSS:4.0/AV:P/AC:H/AT:P/PR:H/UI:A/VC:H/VI:H/VA:H/SC:H/SI:H/SA:H/MAV:N/MAC:H/MAT:N/MPR:L/MUI:A/MVC:N/MVI:L/MVA:L/MSC:H/MSI:N/MSA:L",
                4.5,
            ),
            (
                "CVSS:4.0/AV:P/AC:H/AT:P/PR:H/UI:A/VC:H/VI:H/VA:H/SC:H/SI:H/SA:H/MAV:P/MAC:L/MAT:N/MPR:H/MUI:P/MVC:L/MVI:L/MVA:H/MSC:N/MSI:N/MSA:H",
                6.8,
            ),
            (
                "CVSS:4.0/AV:P/AC:H/AT:P/PR:H/UI:A/VC:H/VI:H/VA:H/SC:H/SI:H/SA:H/MAV:A/MAC:H/MAT:N/MPR:H/MUI:A/MVC:L/MVI:H/MVA:L/MSC:N/MSI:S/MSA:L",
                7.1,
            ),
            (
                "CVSS:4.0/AV:N/AC:L/AT:N/PR:N/UI:N/VC:N/VI:N/VA:N/SC:N/SI:N/SA:N/MAV:P/MAC:L/MAT:N/MPR:L/MUI:A/MVC:H/MVI:N/MVA:N/MSC:N/MSI:H/MSA:S",
                8.2,
            ),
            (
                "CVSS:4.0/AV:L/AC:L/AT:P/PR:H/UI:N/VC:L/VI:L/VA:N/SC:L/SI:N/SA:L",
                1.8,
            ),
            (
                "CVSS:4.0/AV:A/AC:H/AT:N/PR:H/UI:A/VC:N/VI:L/VA:L/SC:L/SI:L/SA:N",
                1.0,
            ),
        ] {
            assert_eq!(score(vector), Some(expected), "{vector}");
        }
    }

    #[test]
    fn lookup_table_is_sorted_for_binary_search() {
        assert!(LOOKUP.windows(2).all(|w| w[0].0 < w[1].0));
    }

    #[test]
    fn rejects_invalid_vectors() {
        for vector in [
            "",
            "garbage",
            // v3 vector
            "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H",
            // missing a base metric (SA)
            "CVSS:4.0/AV:N/AC:L/AT:N/PR:N/UI:N/VC:H/VI:H/VA:H/SC:N/SI:N",
            // invalid value
            "CVSS:4.0/AV:X/AC:L/AT:N/PR:N/UI:N/VC:H/VI:H/VA:H/SC:N/SI:N/SA:N",
            // duplicate metric
            "CVSS:4.0/AV:N/AV:L/AC:L/AT:N/PR:N/UI:N/VC:H/VI:H/VA:H/SC:N/SI:N/SA:N",
            // unknown metric
            "CVSS:4.0/AV:N/AC:L/AT:N/PR:N/UI:N/VC:H/VI:H/VA:H/SC:N/SI:N/SA:N/ZZ:X",
        ] {
            assert_eq!(score(vector), None, "{vector}");
        }
    }
}
