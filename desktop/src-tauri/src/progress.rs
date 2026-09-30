//! Shared, phase-aware progress. Unknown work stays indeterminate; estimates use
//! a recent window rather than counting cached work as newly processed work.
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};

#[derive(Default)]
pub struct Estimate {
    samples: VecDeque<(f64, u64)>,
    pub rate: Option<f64>,
    total: u64,
    phase: String,
    done: u64,
}
impl Estimate {
    pub fn observe(&mut self, now: f64, done: u64, total: u64, phase: &str) {
        if self.total != total || self.phase != phase || done < self.done {
            *self = Self {
                total,
                phase: phase.into(),
                ..Self::default()
            };
        }
        self.done = done.min(total);
        if self.samples.back().is_some_and(|(at, _)| now - at < 0.25) {
            return;
        }
        self.samples.push_back((now, self.done));
        while self.samples.len() > 2 && self.samples.front().is_some_and(|(at, _)| now - at > 30.) {
            self.samples.pop_front();
        }
        if let Some(&(at, count)) = self.samples.front() {
            let elapsed = now - at;
            if elapsed >= 1. && self.done > count {
                let rate = (self.done - count) as f64 / elapsed;
                self.rate = Some(self.rate.map_or(rate, |old| old * 0.8 + rate * 0.2));
            }
        }
    }
    pub fn eta(&self) -> Option<u64> {
        if self.total > 0 && self.done == self.total {
            return Some(0);
        }
        self.rate
            .filter(|r| *r > 0.)
            .map(|r| ((self.total - self.done) as f64 / r).ceil() as u64)
    }
}

/// Accept counts from operational clauses, not a title/path containing “01/02”.
fn counts(message: &str) -> Option<(u64, u64, String)> {
    for clause in message.split('·') {
        let words: Vec<_> = clause.split_whitespace().collect();
        for (i, word) in words.iter().enumerate() {
            let trimmed = word.trim_matches(|c: char| c == '(' || c == ')');
            let Some((left, right)) = trimmed.split_once('/') else {
                continue;
            };
            let Ok(done) = left.replace(',', "").parse::<u64>() else {
                continue;
            };
            let Ok(total) = right.replace(',', "").parse::<u64>() else {
                continue;
            };
            let unit = words.get(i + 1).copied().unwrap_or("");
            if !(matches!(unit, "files" | "tracks" | "releases" | "artists" | "items")
                || words.len() == 1)
            {
                continue;
            }
            if total > 0 && done <= total {
                return Some((done, total, unit.to_owned()));
            }
        }
    }
    None
}

#[derive(Default)]
pub struct Progress {
    jobs: HashMap<String, Estimate>,
}
impl Progress {
    pub fn update(&mut self, job: &mut Value, message: &str, now: f64) {
        let id = job["id"].as_str().unwrap_or("unknown").to_owned();
        let explicit = job["completed"]
            .as_u64()
            .zip(job["total"].as_u64())
            .filter(|(d, t)| *t > 0 && d <= t);
        let measured = explicit
            .map(|(d, t)| {
                (
                    d,
                    t,
                    job["progress_phase_override"].as_str().map(str::to_owned).or_else(|| counts(message)
                        .map(|c| c.2)
                    ).unwrap_or_else(|| "items".to_owned()),
                )
            })
            .or_else(|| counts(message));
        let measured = measured.filter(|(_, _, unit)| match job["kind"].as_str() {
            Some("link") => unit == "tracks",
            Some("discography") => unit == "artists" || unit == "items" || (explicit.is_some() && unit == "reference releases"),
            Some("download") => unit == "tracks",
            _ => true,
        });
        let estimate = self.jobs.entry(id).or_default();
        if let Some((done, total, phase)) = measured {
            estimate.observe(now, done, total, &phase);
        } else if estimate.total > 0 {
            estimate.observe(now, estimate.done, estimate.total, &estimate.phase.clone());
        }
        job["message"] = json!(message);
        job["progress_updated_at"] = json!(now);
        if estimate.total > 0 {
            job["completed"] = json!(estimate.done);
            job["total"] = json!(estimate.total);
            job["percent"] = json!(estimate.done as f64 / estimate.total as f64 * 100.);
            job["eta_seconds"] = json!(estimate.eta());
            job["items_per_second"] = json!(estimate.rate);
            job["progress_phase"] = json!(estimate.phase);
        }
        // Do not mistake fields added by this adapter for producer-supplied counts.
        job["progress_measured"] = json!(true);
    }
    pub fn finish(&mut self, job: &mut Value) {
        if let Some(id) = job["id"].as_str() {
            self.jobs.remove(id);
        }
        if job["status"] == "complete" {
            job["percent"] = json!(100);
            job["eta_seconds"] = json!(0);
        } else {
            job["eta_seconds"] = Value::Null;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn downloaded_references_have_their_own_measured_phase() {
        let mut progress = Progress::default();
        let mut job = json!({"id":"refresh","kind":"discography","completed":5,"total":10,"progress_phase_override":"reference releases"});
        progress.update(&mut job,"Downloaded reference metadata · 5/10 releases",1.);
        assert_eq!(job["percent"],50.);
        assert_eq!(job["progress_phase"],"reference releases");
        job.as_object_mut().unwrap().remove("progress_phase_override");
        job["completed"]=json!(1);job["total"]=json!(20);
        progress.update(&mut job,"Checking release lists · 1/20 artists",2.);
        assert_eq!(job["percent"],5.);
        assert_eq!(job["progress_phase"],"artists");
        assert!(job["eta_seconds"].is_null());
    }
    #[test]
    fn cached_start_and_phases_do_not_inflate_speed() {
        let mut e = Estimate::default();
        e.observe(0., 800, 1000, "tracks");
        assert_eq!(e.eta(), None);
        e.observe(10., 810, 1000, "tracks");
        assert_eq!(e.eta(), Some(190));
        e.observe(11., 820, 1000, "tracks");
        assert!(e.eta().unwrap() > 140); // a single fast burst cannot slash ETA
        e.observe(12., 1, 10, "releases");
        assert_eq!(e.eta(), None);
        e.observe(15., 10, 10, "releases");
        assert_eq!(e.eta(), Some(0));
    }
    #[test]
    fn nested_release_counts_cannot_reset_track_progress_and_jobs_are_isolated() {
        let mut p = Progress::default();
        let mut j = json!({"id":"link","kind":"link"});
        p.update(&mut j, "Linked · 2/20 tracks checked", 0.);
        let mut nested = json!({"id":"link","kind":"link"});
        p.update(&mut nested, "Loading · 1/3 releases", 3.);
        assert_eq!(nested["total"], 20);
        assert_eq!(nested["completed"], 2);
        let mut other = json!({"id":"other","kind":"scan"});
        p.update(&mut other, "Reading · 1/10 files", 4.);
        assert_eq!(other["total"], 10);
        nested["status"] = json!("cancelled");
        p.finish(&mut nested);
        assert!(nested["eta_seconds"].is_null());
        assert_eq!(p.jobs.len(), 1);
    }
    #[test]
    fn operational_counts_only_and_unknown_total() {
        assert!(counts("Searching: Artist 01/02 — Song").is_none());
        assert_eq!(
            counts("Linked · 12/20 tracks checked · Artist").unwrap().0,
            12
        );
        assert!(counts("Scanning files").is_none());
        let mut p = Progress::default();
        let mut j = json!({"id":"a"});
        p.update(&mut j, "Discovering files", 0.);
        assert!(j["percent"].is_null());
        p.update(&mut j, "Checking · 4/10 files", 1.);
        assert_eq!(j["percent"], 40.);
    }
}
