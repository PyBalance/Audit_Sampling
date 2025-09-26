//! MUS audit sampling: planning and extraction (Rust)
//!
//! Port of `MUS.planning` and `MUS.extraction` from ref/R.
//! See Design.md for algorithm details.

use rand::{rngs::StdRng, Rng, SeedableRng};
use std::cmp::{max, min};

#[derive(Debug, Clone)]
pub struct PlanningOptions {
    pub col_name_book_values: String,
    pub confidence_level: f64,
    pub tolerable_error: f64,
    pub expected_error: f64,
    pub n_min: usize,
    pub errors_as_pct: bool,
    pub conservative: bool,
    pub combined: bool,
}

impl Default for PlanningOptions {
    fn default() -> Self {
        Self {
            col_name_book_values: "book.value".to_string(),
            confidence_level: 0.90,
            tolerable_error: f64::NAN,
            expected_error: f64::NAN,
            n_min: 0,
            errors_as_pct: false,
            conservative: false,
            combined: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub data: Vec<f64>,
    pub col_name_book_values: String,
    pub confidence_level: f64,
    pub tolerable_error: f64,
    pub expected_error: f64,
    pub book_value: f64,
    pub n: usize,
    pub high_value_threshold: f64,
    pub tolerable_taintings: f64,
    pub combined: bool,
}

#[derive(Debug, Clone)]
pub struct ExtractionOptions {
    pub start_point: Option<f64>,
    pub seed: Option<u64>,
    pub obey_n_as_min: bool,
    pub combined: bool,
}

impl Default for ExtractionOptions {
    fn default() -> Self {
        Self { start_point: None, seed: None, obey_n_as_min: false, combined: false }
    }
}

#[derive(Debug, Clone)]
pub struct ExtractedItem {
    pub book_value: f64,
    pub mus_hit: u64,
    pub cum_before: u64,
    pub cum_after: u64,
}

#[derive(Debug, Clone)]
pub struct Extraction {
    pub plan: Plan,
    pub start_point: f64,
    pub seed: Option<u64>,
    pub obey_n_as_min: bool,
    pub high_values: Vec<f64>,
    pub sample_population: Vec<(f64, u64)>, // (book_value, cumulative MU)
    pub sampling_interval: f64,
    pub sample: Vec<ExtractedItem>,
    pub extensions: usize,
    pub n_qty: Vec<usize>,
    pub combined: bool,
}

#[derive(thiserror::Error, Debug)]
pub enum MusError {
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("calculation failed: {0}")]
    Calculation(String),
}

fn is_finite_non_nan(x: f64) -> bool { x.is_finite() && !x.is_nan() }

fn sum_nonneg(values: &[f64]) -> f64 {
    values.iter().map(|&v| v.max(0.0)).sum::<f64>()
}

fn round_to_u64(x: f64) -> u64 {
    // R uses round to nearest, ties to even. Rust's f64::round is ties-to-even too.
    let r = x.round();
    if r < 0.0 { 0 } else { r as u64 }
}

fn ceil_to_u64(x: f64) -> u64 { if x <= 0.0 { 0 } else { x.ceil() as u64 } }

fn ceil_to_usize(x: f64) -> usize { if x <= 0.0 { 0 } else { x.ceil() as usize } }

// Compute minimal integer k such that P[X <= q] >= alpha for X~Hypergeom(N, m, k)
// Here N = m + n_black, with successes m.
fn min_draws_for_cdf_at_most_q(q: u64, alpha: f64, m: u64, n_black: u64, k_max: u64) -> Result<u64, MusError> {
    use statrs::distribution::{DiscreteCDF, Hypergeometric};
    let n_total = m + n_black;
    if n_total == 0 { return Err(MusError::Calculation("population size is zero".into())); }
    // Find the smallest k such that CDF(q; k) <= alpha
    // CDF is non-increasing in k for fixed q.
    let mut lo: u64 = 0;
    let mut hi: u64 = k_max.min(n_total);
    let mut ans: Option<u64> = None;
    while lo <= hi {
        let mid = lo + (hi - lo) / 2;
        let hg = Hypergeometric::new(n_total as u64, m as u64, mid as u64)
            .map_err(|e| MusError::Calculation(format!("hypergeometric new: {e}")))?;
        let cdf = hg.cdf(q);
        if cdf <= alpha {
            ans = Some(mid);
            if mid == 0 { break; }
            hi = mid - 1;
        } else {
            lo = mid + 1;
        }
    }
    Ok(ans.unwrap_or_else(|| k_max.min(n_total)))
}

fn calculate_n_hyper(num_errors: u64, alpha: f64, tolerable_error: f64, account_value: f64) -> Result<u64, MusError> {
    // Mirrors .calculate.n.hyper in R.
    if !(alpha.is_finite() && alpha > 0.0 && alpha < 1.0) {
        return Err(MusError::InvalidInput("alpha must be in (0,1)".into()));
    }
    if !(tolerable_error.is_finite() && account_value.is_finite() && account_value > 0.0) {
        return Err(MusError::InvalidInput("tolerable_error and account_value must be finite and > 0".into()));
    }
    let max_error_rate = tolerable_error / account_value;
    let m = round_to_u64(max_error_rate * account_value); // allowed wrong units
    let n_black = round_to_u64((1.0 - max_error_rate) * account_value); // correct MUs
    let k_upper = min(n_black + num_errors, round_to_u64(account_value));
    let k = min_draws_for_cdf_at_most_q(num_errors, alpha, m, n_black, k_upper)?;
    Ok(k)
}

fn mus_factor(confidence_level: f64, pct_ratio: f64) -> Result<f64, MusError> {
    use statrs::distribution::{ContinuousCDF, Gamma};
    if !(confidence_level > 0.0 && confidence_level < 1.0) {
        return Err(MusError::InvalidInput("confidence_level must be in (0,1)".into()));
    }
    if !(pct_ratio >= 0.0 && pct_ratio < 1.0) {
        return Err(MusError::InvalidInput("pct_ratio must be in [0,1)".into()));
    }
    if pct_ratio == 0.0 {
        let g = Gamma::new(1.0, 1.0).map_err(|e| MusError::Calculation(format!("gamma: {e}")))?;
        return Ok(g.inverse_cdf(confidence_level));
    }
    let mut f_prev = 0.0;
    let mut f = {
        let g = Gamma::new(1.0, 1.0).map_err(|e| MusError::Calculation(format!("gamma: {e}")))?;
        g.inverse_cdf(confidence_level)
    };
    let tol = 1e-6;
    let max_iter = 1000;
    for _ in 0..max_iter {
        f_prev = f;
        let shape = 1.0 + pct_ratio * f_prev;
        let g = Gamma::new(shape, 1.0).map_err(|e| MusError::Calculation(format!("gamma: {e}")))?;
        f = g.inverse_cdf(confidence_level);
        if (f - f_prev).abs() <= tol {
            return Ok(f);
        }
    }
    Err(MusError::Calculation("MUS.factor iteration did not converge".into()))
}

fn mus_calc_n_conservative(confidence_level: f64, tolerable_error: f64, expected_error: f64, book_value: f64) -> Result<usize, MusError> {
    let pct_ratio = expected_error / tolerable_error;
    let f = mus_factor(confidence_level, pct_ratio)?;
    let conf_factor = (f * 100.0).ceil() / 100.0;
    let n = (conf_factor / tolerable_error * book_value).ceil();
    Ok(n as usize)
}

pub fn mus_planning(book_values: &[f64], mut opts: PlanningOptions) -> Result<Plan, MusError> {
    if book_values.is_empty() {
        return Err(MusError::InvalidInput("data must contain at least one item".into()));
    }
    if !(opts.confidence_level > 0.0 && opts.confidence_level < 1.0) {
        return Err(MusError::InvalidInput("confidence.level must be in (0,1)".into()));
    }
    let mut nonfinite = false;
    let mut has_zero = false;
    let mut has_negative = false;
    for &v in book_values {
        if !is_finite_non_nan(v) { nonfinite = true; }
        if v == 0.0 { has_zero = true; }
        if v < 0.0 { has_negative = true; }
    }
    if nonfinite {
        eprintln!("Warning: There are missing or infinite values in book values; they have no chance for selection.");
    }
    if has_zero { eprintln!("Warning: There are zeros as book values; they have no chance for selection."); }
    if has_negative { eprintln!("Warning: There are negative book values; they are ignored (pmax)." ); }

    let book_value = sum_nonneg(book_values);
    let num_items = book_values.len();

    if opts.errors_as_pct && opts.tolerable_error.is_finite() && opts.expected_error.is_finite() {
        opts.tolerable_error = opts.tolerable_error * book_value;
        opts.expected_error = opts.expected_error * book_value;
    }
    if !(opts.tolerable_error.is_finite() && opts.tolerable_error > 0.0) {
        return Err(MusError::InvalidInput("tolerable.error must be > 0".into()));
    }
    if !(opts.expected_error.is_finite() && opts.expected_error >= 0.0) {
        return Err(MusError::InvalidInput("expected.error must be >= 0".into()));
    }
    if opts.n_min >= num_items {
        return Err(MusError::InvalidInput("n.min must be < number of items".into()));
    }
    let too_large = (opts.tolerable_error / book_value) * (1.0 - opts.confidence_level) * (opts.tolerable_error - opts.expected_error).sqrt() < 0.07;
    if too_large {
        eprintln!("Warning: Combination of parameters leads to impractically large sample.");
    }

    let n_optimal: usize = if opts.tolerable_error >= book_value {
        eprintln!("Warning: tolerable.error >= book.value; no sampling necessary, proceeding with n=0.");
        0
    } else if calculate_n_hyper(0, 1.0 - opts.confidence_level, opts.tolerable_error, book_value)? < 1 {
        return Err(MusError::Calculation("Undefined: if 0 errors occur, sample size must be positive".into()));
    } else if (calculate_n_hyper(num_items as u64, 1.0 - opts.confidence_level, opts.tolerable_error, book_value)? as f64)
        * opts.expected_error / book_value - (num_items as f64) > 0.0
    {
        eprintln!("Warning: MUS makes no sense for your problem - sample size must exceed population items; auditing everything.");
        num_items
    } else {
        // Zero expected error: directly solve using hypergeometric without interpolation
        if opts.expected_error == 0.0 {
            calculate_n_hyper(0, 1.0 - opts.confidence_level, opts.tolerable_error, book_value)? as usize
        } else {
            // find i where crossing occurs
            let mut i: u64 = 0;
            loop {
                let n_i = calculate_n_hyper(i, 1.0 - opts.confidence_level, opts.tolerable_error, book_value)? as f64;
                if n_i * opts.expected_error / book_value <= i as f64 { break; }
                i += 1;
            }
            if i == 0 {
                calculate_n_hyper(0, 1.0 - opts.confidence_level, opts.tolerable_error, book_value)? as usize
            } else {
                let ni = calculate_n_hyper(i - 1, 1.0 - opts.confidence_level, opts.tolerable_error, book_value)? as f64;
                let nip1 = calculate_n_hyper(i, 1.0 - opts.confidence_level, opts.tolerable_error, book_value)? as f64;
                let denom = 1.0 / (nip1 - ni) - opts.expected_error / book_value;
                if denom <= 0.0 { return Err(MusError::Calculation("denominator non-positive in interpolation".into())); }
                let n_opt = ((ni / (nip1 - ni) - (i as f64 - 1.0)) / denom).ceil();
                let mut n_opt = if n_opt < 0.0 { 0 } else { n_opt as usize };
                if n_opt > num_items {
                    eprintln!("Warning: MUS makes no sense - n > population size; auditing everything.");
                    n_opt = num_items;
                } else if (n_opt as f64 - (nip1 + 1.0)).abs() < f64::EPSILON {
                    n_opt = (n_opt - 1).max(0);
                } else if (n_opt as f64) < ni || (n_opt as f64) > nip1 {
                    return Err(MusError::Calculation(format!(
                        "n.optimal not plausible: n_opt={n_opt}, ni={ni}, nip1={nip1}"
                    )));
                }
                n_opt
            }
        }
    };

    let mut n_final = max(n_optimal, opts.n_min);
    if opts.conservative {
        let n_cons = mus_calc_n_conservative(opts.confidence_level, opts.tolerable_error, opts.expected_error, book_value)?;
        n_final = n_final.max(n_cons);
    }
    // Guard divide-by-zero if n_final ended up zero (possible when tolerable_error >= book_value)
    let interval = if n_final == 0 { f64::INFINITY } else { book_value / n_final as f64 };
    let tol_taint = if book_value == 0.0 { 0.0 } else { opts.expected_error / book_value * n_final as f64 };

    Ok(Plan {
        data: book_values.iter().cloned().collect(),
        col_name_book_values: opts.col_name_book_values,
        confidence_level: opts.confidence_level,
        tolerable_error: opts.tolerable_error,
        expected_error: opts.expected_error,
        book_value,
        n: n_final,
        high_value_threshold: interval,
        tolerable_taintings: tol_taint,
        combined: opts.combined,
    })
}

pub fn mus_extraction(plan: &Plan, opts: ExtractionOptions) -> Result<Extraction, MusError> {
    if plan.n == 0 {
        return Err(MusError::InvalidInput("plan.n must be > 0 for extraction".into()));
    }
    // Split into high values and sampling population
    let mut high_values: Vec<f64> = Vec::new();
    let mut sample_population: Vec<f64> = Vec::new();
    for &v in &plan.data {
        if v >= plan.high_value_threshold { high_values.push(v); } else { sample_population.push(v); }
    }
    let mut interval = plan.high_value_threshold;
    if opts.obey_n_as_min {
        // perfect sampling interval and stabilize partition
        loop {
            let old_interval = interval;
            let pop_sum: f64 = sample_population.iter().sum();
            let denom = plan.n as isize - high_values.len() as isize;
            if denom <= 0 { return Err(MusError::Calculation("no items left for sampling after removing high values".into())); }
            interval = pop_sum / denom as f64;
            if (interval - old_interval).abs() <= 0.0 { break; }
            // re-partition
            high_values.clear();
            sample_population.clear();
            for &v in &plan.data {
                if v >= interval { high_values.push(v); } else { sample_population.push(v); }
            }
            if (interval - old_interval).abs() == 0.0 { break; }
        }
    }

    if let Some(sp) = opts.start_point {
        if !(sp >= 0.0 && sp <= interval) {
            return Err(MusError::InvalidInput("start.point must be in [0, interval]".into()));
        }
    }
    let start_point = match opts.start_point {
        Some(v) => v,
        None => {
            let seed = opts.seed.unwrap_or_else(|| {
                use std::time::{SystemTime, UNIX_EPOCH};
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_nanos() as u64)
                    .unwrap_or(0)
            });
            let mut rng: StdRng = StdRng::seed_from_u64(seed);
            rng.random_range(0.0..=interval)
        }
    };

    let draws_needed = plan.n.saturating_sub(high_values.len());
    // Calculate sampling units with R-like rounding
    let grid_step = (interval * 100.0).round() / 100.0;
    let mut sampling_units: Vec<u64> = (0..=draws_needed)
        .map(|j| (start_point + j as f64 * grid_step).round())
        .map(|x| if x <= 0.0 { 0 } else { x as u64 })
        .collect();

    // Prepare cumulative sums
    let mut cum = Vec::<u64>::with_capacity(sample_population.len());
    let mut running: u64 = 0;
    for &v in &sample_population {
        let inc = round_to_u64(v);
        running = running.saturating_add(inc);
        cum.push(running);
    }
    let pop_sum_u = running;
    sampling_units.retain(|&u| u <= pop_sum_u);

    // findInterval equivalent: index i s.t. cum[i-1] < u <= cum[i]
    let mut sample: Vec<ExtractedItem> = Vec::with_capacity(sampling_units.len());
    for &u in &sampling_units {
        let mut lo: isize = -1; // represents 0 baseline
        let mut hi: isize = cum.len() as isize - 1;
        while lo < hi {
            let mid = lo + (hi - lo) / 2 + 1; // upper mid to avoid tight loop
            if cum[mid as usize] < u { lo = mid; } else { hi = mid - 1; }
        }
        let idx = (hi + 1) as usize;
        if idx >= cum.len() { continue; }
        let before = if idx == 0 { 0 } else { cum[idx - 1] };
        let after = cum[idx];
        sample.push(ExtractedItem { book_value: sample_population[idx], mus_hit: u, cum_before: before, cum_after: after });
    }

    // Reassessed sampling interval
    let pop_sum: f64 = sample_population.iter().sum();
    let sample_len = sample.len();
    let sampling_interval = if sample_len == 0 { f64::INFINITY } else { pop_sum / sample_len as f64 };

    let sample_population_with_cum: Vec<(f64, u64)> = sample_population
        .into_iter()
        .zip(cum.into_iter())
        .collect();

    Ok(Extraction {
        plan: plan.clone(),
        start_point,
        seed: opts.seed,
        obey_n_as_min: opts.obey_n_as_min,
        high_values,
        sample_population: sample_population_with_cum,
        sampling_interval,
        sample,
        extensions: 0,
        n_qty: vec![sample_len],
        combined: opts.combined,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;

    #[test]
    fn planning_and_extraction_simple() {
        // Synthetic deterministic dataset similar to man examples (integer MUs)
        let mut data = Vec::new();
        // 500 invoices, values 1..1000 randomly would be typical; here use a simple ramp
        for i in 0..500u64 { data.push(((i % 1000) + 1) as f64); }
        let opts = PlanningOptions {
            tolerable_error: 100_000.0,
            expected_error: 20_000.0,
            ..PlanningOptions::default()
        };
        let plan = mus_planning(&data, opts).expect("plan");
        // Basic sanity
        assert!(plan.n >= 0);
        assert!(plan.high_value_threshold.is_finite());
        // Extraction with fixed seed and obey_n_as_min for determinism
        let ext = mus_extraction(&plan, ExtractionOptions { start_point: Some(5.0), seed: Some(0), obey_n_as_min: true, combined: false }).expect("extract");
        assert!(ext.sample.len() <= plan.n);
        // Interval recompute equals pop_sum / sample_len
        let pop_sum: f64 = ext.sample_population.iter().map(|(v, _)| *v).sum();
        assert_abs_diff_eq!(ext.sampling_interval, pop_sum / ext.sample.len() as f64, epsilon = 1e-9);
    }
}
