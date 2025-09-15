Design: MUS Planning and Extraction (Rust)

Overview
- Goal: Reimplement the MUS sample planning and extraction functions from ref/R (MUS.planning and MUS.extraction) in Rust with numerically equivalent behavior for typical inputs.
- Source reference: ref/R/MUS.planning.R and ref/R/MUS.extraction.R in this repo.
- Deliverables: Rust library with `mus_planning` and `mus_extraction` in `src/`, unit tests colocated with the code, and latest crates added via `cargo add`.

Terminology
- book value: monetary unit value per population item. Planning/extraction only use this column from the data frame.
- N (population MUs): sum of non‑negative book values (R uses pmax(book.value, 0)). R docs advise integer units (e.g., cents) so no decimals.
- n: planned sample size.
- High value threshold: equal to sampling interval; items with book value ≥ threshold are audited as individually significant ("high values").

Rust API
- `mus_planning(book_values: &[f64], options: PlanningOptions) -> Result<Plan>`
  - Computes `n`, `High.value.threshold`, `tolerable.taintings`, etc., mirroring MUS.planning.
- `mus_extraction(plan: &Plan, options: ExtractionOptions) -> Result<Extraction>`
  - Splits into `high_values` and `sample_population`, performs fixed-interval selection, and returns the sample and revised interval as in MUS.extraction.

Crates
- statrs: hypergeometric CDF and gamma inverse CDF (qgamma) equivalents.
- rand: RNG for start point when none is provided, reproducible via seed.
- approx: tolerant float comparisons in tests.

Planning Algorithm (parity with MUS.planning)
Inputs
- data: vector of book values (non‑negative considered; negatives ignored per R’s `pmax`).
- col name is tracked for parity but not used in numeric calculations.
- confidence.level ∈ (0,1), tolerable.error > 0, expected.error ≥ 0, n.min ≥ 0, errors.as.pct flag, conservative flag, combined flag.

Preprocessing
1) Validate inputs, warn conditions as in R (not fatal):
   - Any non‑finite, zero, or negative book values → warnings; negatives are excluded from N by max(.,0).
2) Compute:
   - `book_value = sum(max(book_value_i, 0))` (N, integer recommended).
   - `num_items = len(data)`.
   - If `errors.as.pct`, scale tolerable/expected by `book_value`.

Key helper: .calculate.n.hyper(num.errors, alpha, tolerable.error, account.value)
- R uses `phyper(q=num.errors, m=round(max.error.rate*account.value), n=correct.mu, k=n.stichprobe) - alpha` and uniroot over `k` in `[0, min(correct.mu+num.errors, account.value)]`, returning `ceil(root)`.
- Rust approach (discrete, monotone):
  - Let `p = tolerable.error / account.value`.
  - Let `m = round(p * account.value)`, `n_black = round((1 - p) * account.value)`, `N = m + n_black`.
  - Find the minimal integer `k` in `[0, min(n_black + num.errors, N)]` such that `CDF_Hypergeom(q=num.errors; m, n_black, k) ≥ alpha` using binary search.
  - This is equivalent to `ceil(uniroot(...))` for integer outputs and avoids relying on non‑integer `k` behavior.

Compute `n` (R’s branching preserved)
1) If `tolerable.error ≥ book_value`: warn and set `n.optimal = 0`.
2) Else if `.calculate.n.hyper(0) < 0`: error (undefined in R; we match with a Rust error).
3) Else if `.calculate.n.hyper(num_items) * expected.error / book_value - num_items > 0`:
     - warn that MUS makes no sense and set `n.optimal = num_items`.
4) Else:
   - Find smallest `i >= 0` with `.calculate.n.hyper(i) * expected.error / book_value ≤ i`.
   - Let `ni = .calculate.n.hyper(i-1)`, `nip1 = .calculate.n.hyper(i)`.
   - Linear interpolation (R code):
     `n.optimal = ceil((ni/(nip1 - ni) - (i - 1)) / (1/(nip1 - ni) - expected.error/book_value))`.
   - Plausibility checks (same as R):
     - if `n.optimal > num_items` → warn and set `n.optimal = num_items`.
     - if `n.optimal == nip1 + 1` → `n.optimal -= 1`.
     - else enforce `ni ≤ n.optimal ≤ nip1` (panic in R; in Rust we return an error with context).
5) `n.final = max(n.optimal, n.min)`.
6) Conservative override (MUS.calc.n.conservative):
   - `pct.ratio = expected.error / tolerable.error`.
   - `conf.factor = ceil(MUS.factor(confidence.level, pct.ratio)*100)/100`.
   - `n.cons = ceil(conf.factor / tolerable.error * book_value)`.
   - `n.final = max(n.final, n.cons)` if `conservative`.
7) Derived outputs:
   - `interval = book_value / n.final` (High.value.threshold in planning result).
   - `tolerable.taintings = expected.error / book_value * n.final`.

MUS.factor (qgamma-based fixed point)
- If `pct.ratio == 0`: `F = qgamma(confidence.level, shape=1, scale=1)`.
- Else iterate: `F_{t+1} = qgamma(confidence.level, shape = 1 + pct.ratio * F_t, scale=1)` until `|F_{t+1} - F_t| ≤ tol` or `max_iter`.
- Use `statrs::distribution::Gamma` with `inverse_cdf`.

Extraction Algorithm (parity with MUS.extraction)
Inputs
- `plan` from Rust planning. Options: `start_point: Option<f64>`, `seed: Option<u64>`, `obey_n_as_min: bool`, `combined` passthrough.

Steps
1) Partition into `high_values = {x | x ≥ High.value.threshold}` and `sample_population = {x | x < High.value.threshold}`.
2) Set `interval = High.value.threshold` by default.
3) If `obey_n_as_min` is true, compute the perfect interval
   - `interval = sum(sample_population) / (plan.n - high_values.len())`.
   - If this changes the threshold (i.e., more items become high values), re-partition and recompute until stable (same loop as R while(oldinterval != interval)).
4) Validate `start_point` in `[0, interval]`. If None, draw U[0, interval] using `rand` with optional `seed`.
5) Build sampling units (exact rounding behavior):
   - `grid_step = round(interval, 2)`; `sampling_units = round(start_point + j * grid_step)` for j = 0..(plan.n - high_values.len()).
   - Keep only units ≤ sum(sample_population).
6) Compute cumulative sums `cum = cumsum(sample_population)` and select index i where `cum[i-1] < u ≤ cum[i]` for each `u` (R `findInterval` with left-open [0, cum]).
7) Extract those items as `sample` and record the hit `u` as `mus_hit`.
8) Reassess interval for evaluation: `interval_eval = sum(sample_population) / sample.len()`.
9) Return plan fields + extraction fields, matching R names semantically.

Behavioral Parity Notes
- R expects discrete MUs; tests use integer-valued book values (e.g., cents). The Rust code treats inputs as f64 but rounds where the R code does, and uses integer arithmetic internally for hypergeometric parameters.
- The discrete binary search for `.calculate.n.hyper` yields the minimal integer `k` that achieves the CDF bound, which matches `ceil(uniroot(...))` for integer outcomes.
- R’s warning/stop messages are mapped to Rust `Warning` logs (not fatal) and `Error` returns where R stops.

Tests
- Unit tests live alongside code (in-module `#[cfg(test)]`).
- We include deterministic cases using small synthetic populations and seeds; expected values were derived by running the R reference implementation (documented in comments) and asserted in Rust within tolerances for floating-point fields.
- Numeric comparisons use `approx` with sensible epsilons for interval rounding, taintings, and gamma-based factor.

Limitations
- If inputs contain NaN/Inf or many negatives, Rust rejects with errors where R only warns. This is documented in function docs.
- If the conservative gamma iteration fails to converge within max_iter, Rust returns an error equivalent to R’s `erro` value path.

