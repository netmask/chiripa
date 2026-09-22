//! All of chiripa's math lives here, with tests.
//!
//! CSV parsing, 6-of-56 era statistics (frequencies, gaps, co-occurrences,
//! sum percentiles) and the 8 ticket-generation strategies. The browser only
//! formats what this module computes.

use rand::{Rng, RngExt};

/// Window of draws considered "recent".
pub const WINDOW: usize = 250;
/// Strategies that use no randomness: same result until new draws arrive.
pub const DETERMINISTIC: [&str; 2] = ["hot", "cold"];
pub const STRATEGIES: [&str; 8] = [
    "hot", "cold", "weighted", "recent", "bayes", "balanced", "sterncover", "markov",
];

pub struct Draw {
    pub contest: u32,
    pub date: String,
    pub numbers: [u8; 6],
    pub pot: u64,
}

/// Draws of the 6-of-56 era (from the first contest containing a number
/// >= 50), oldest to newest. R7 (the extra ball) is ignored.
pub fn parse(csv: &str) -> Vec<Draw> {
    let mut draws: Vec<Draw> = csv
        .lines()
        .skip(1)
        .filter_map(|line| {
            let c: Vec<&str> = line.trim().split(',').collect();
            if c.len() < 11 {
                return None;
            }
            let contest = c[1].parse().ok()?;
            let mut numbers = [0u8; 6];
            for (i, cell) in c[2..8].iter().enumerate() {
                numbers[i] = cell.parse().ok().filter(|n| (1..=56).contains(n))?;
            }
            numbers.sort_unstable();
            let pot = c[9].trim().parse().unwrap_or(0);
            Some(Draw { contest, date: c[10].to_string(), numbers, pot })
        })
        .collect();
    draws.sort_by_key(|d| d.contest);
    if let Some(start) = draws.iter().position(|d| d.numbers.iter().any(|&n| n >= 50)) {
        draws.drain(..start);
    }
    draws
}

/// Era statistics. Arrays are indexed by ball number (1..=56); index 0 unused.
pub struct Stats {
    pub n: usize,
    pub contest: u32,
    pub date: String,
    pub last: [u8; 6],
    pub last_pot: u64,
    pub freq: [u32; 57],
    pub recent: [u32; 57],
    pub gap: [u32; 57],
    cooc: Vec<u32>, // 57 * 57, symmetric
    pub sum_lo: u32,
    pub sum_hi: u32,
    pub max_gap: u32,
}

impl Stats {
    pub fn new(draws: &[Draw]) -> Self {
        let n = draws.len();
        let mut freq = [0u32; 57];
        let mut recent = [0u32; 57];
        let mut gap = [n as u32; 57];
        let mut cooc = vec![0u32; 57 * 57];
        let mut sums: Vec<u32> = Vec::with_capacity(n);
        for (i, d) in draws.iter().enumerate() {
            let mut sum = 0u32;
            for &a in &d.numbers {
                sum += a as u32;
                freq[a as usize] += 1;
                gap[a as usize] = (n - 1 - i) as u32;
                if i >= n.saturating_sub(WINDOW) {
                    recent[a as usize] += 1;
                }
                for &b in &d.numbers {
                    if a != b {
                        cooc[a as usize * 57 + b as usize] += 1;
                    }
                }
            }
            sums.push(sum);
        }
        sums.sort_unstable();
        let pct = |p: f64| sums[((n as f64 * p) as usize).min(n.saturating_sub(1))];
        Stats {
            n,
            contest: draws.last().map(|d| d.contest).unwrap_or(0),
            date: draws.last().map(|d| d.date.clone()).unwrap_or_default(),
            last: draws.last().map(|d| d.numbers).unwrap_or_default(),
            last_pot: draws.last().map(|d| d.pot).unwrap_or(0),
            freq,
            recent,
            gap,
            cooc,
            sum_lo: pct(0.15),
            sum_hi: pct(0.85),
            max_gap: (1..=56).map(|i| gap[i]).max().unwrap_or(0),
        }
    }

    pub fn cooc(&self, a: u8, b: u8) -> u32 {
        self.cooc[a as usize * 57 + b as usize]
    }

    /// Denominator of the weighted strategy: sum(freq[n] + 1).
    pub fn f_den(&self) -> u32 {
        6 * self.n as u32 + 56
    }

    /// Denominator of the recent strategy: sum(recent[n] + 1).
    pub fn r_den(&self) -> u32 {
        6 * WINDOW.min(self.n) as u32 + 56
    }

    /// Rank of each number by frequency (1 = hottest; ties by lower number).
    pub fn rank(&self) -> [u32; 57] {
        let mut order: Vec<u8> = (1..=56).collect();
        order.sort_by_key(|&x| (std::cmp::Reverse(self.freq[x as usize]), x));
        let mut r = [0u32; 57];
        for (i, &x) in order.iter().enumerate() {
            r[x as usize] = i as u32 + 1;
        }
        r
    }

    /// Stern-Cover popularity model: birthdays and "lucky" numbers.
    pub fn popularity(n: u8) -> f64 {
        let mut p = 1.0;
        if n <= 31 {
            p += 0.8;
        }
        if n <= 12 {
            p += 0.5;
        }
        if [3, 7, 13, 21].contains(&n) {
            p += 0.4;
        }
        p
    }

    pub fn stern_weight(n: u8) -> f64 {
        1.0 / (Self::popularity(n) * Self::popularity(n))
    }

    /// Balanced-strategy weight: (freq + 1) * (1 + gap / max gap).
    pub fn balanced_weight(&self, n: u8) -> f64 {
        (self.freq[n as usize] + 1) as f64
            * (1.0 + self.gap[n as usize] as f64 / self.max_gap.max(1) as f64)
    }

    /// The partner drawn together with n most often.
    pub fn best_pair(&self, n: u8) -> (u8, u32) {
        (1..=56u8)
            .filter(|&m| m != n)
            .map(|m| (m, self.cooc(n, m)))
            .max_by_key(|&(m, v)| (v, std::cmp::Reverse(m)))
            .unwrap_or((0, 0))
    }
}

/// Exact binomial coefficient C(n, r).
pub fn comb(n: u64, r: u64) -> u64 {
    if r > n {
        return 0;
    }
    let mut x: u64 = 1;
    for i in 1..=r {
        x = x * (n - r + i) / i;
    }
    x
}

/// Gamma(shape, 1) for integer shape: sum of `shape` exponentials.
fn gamma_int<R: Rng>(shape: u32, rng: &mut R) -> f64 {
    let mut s = 0.0;
    for _ in 0..shape {
        let u: f64 = rng.random();
        s -= (1.0 - u).ln();
    }
    s
}

/// k distinct numbers, probability proportional to weights[n] (index = number).
fn weighted_sample<R: Rng>(weights: &[f64; 57], k: usize, rng: &mut R) -> Vec<u8> {
    let mut pool: Vec<u8> = (1..=56).collect();
    let mut out = Vec::with_capacity(k);
    for _ in 0..k {
        let total: f64 = pool.iter().map(|&n| weights[n as usize]).sum();
        let mut x = rng.random_range(0.0..total);
        let mut picked = *pool.last().unwrap();
        for &n in &pool {
            x -= weights[n as usize];
            if x <= 0.0 {
                picked = n;
                break;
            }
        }
        out.push(picked);
        pool.retain(|&n| n != picked);
    }
    out.sort_unstable();
    out
}

/// The k highest-scoring numbers (ties: lower number first), sorted.
fn top_k(score: &[f64; 57], k: usize) -> Vec<u8> {
    let mut order: Vec<u8> = (1..=56).collect();
    order.sort_by(|&a, &b| {
        score[b as usize]
            .partial_cmp(&score[a as usize])
            .unwrap()
            .then(a.cmp(&b))
    });
    let mut out: Vec<u8> = order.into_iter().take(k).collect();
    out.sort_unstable();
    out
}

fn weights_from<F: FnMut(u8) -> f64>(mut f: F) -> [f64; 57] {
    let mut w = [0.0; 57];
    for n in 1..=56u8 {
        w[n as usize] = f(n);
    }
    w
}

/// Generate one k-number ticket with the given strategy.
pub fn generate<R: Rng>(st: &Stats, id: &str, k: usize, rng: &mut R) -> Vec<u8> {
    match id {
        "hot" => top_k(&weights_from(|n| st.freq[n as usize] as f64), k),
        "cold" => top_k(&weights_from(|n| st.gap[n as usize] as f64), k),
        "weighted" => {
            weighted_sample(&weights_from(|n| (st.freq[n as usize] + 1) as f64), k, rng)
        }
        "recent" => {
            weighted_sample(&weights_from(|n| (st.recent[n as usize] + 1) as f64), k, rng)
        }
        "bayes" => {
            let w = weights_from(|n| gamma_int(st.freq[n as usize] + 1, rng));
            top_k(&w, k)
        }
        "balanced" => {
            let weights = weights_from(|n| st.balanced_weight(n));
            let lo = (st.sum_lo as f64 * k as f64 / 6.0).round() as u32;
            let hi = (st.sum_hi as f64 * k as f64 / 6.0).round() as u32;
            let p_min = k / 3;
            let p_max = (2 * k).div_ceil(3);
            for _ in 0..8000 {
                let c = weighted_sample(&weights, k, rng);
                let sum: u32 = c.iter().map(|&n| n as u32).sum();
                let evens = c.iter().filter(|&&n| n % 2 == 0).count();
                let lows = c.iter().filter(|&&n| n <= 28).count();
                if (lo..=hi).contains(&sum)
                    && (p_min..=p_max).contains(&evens)
                    && (p_min..=p_max).contains(&lows)
                {
                    return c;
                }
            }
            weighted_sample(&weights, k, rng)
        }
        "sterncover" => {
            let weights = weights_from(Stats::stern_weight);
            let min_high = (2 * k).div_ceil(3);
            for _ in 0..8000 {
                let c = weighted_sample(&weights, k, rng);
                let high = c.iter().filter(|&&n| n >= 32).count();
                let consecutive = c.windows(2).any(|w| w[1] - w[0] == 1);
                if high >= min_high && !consecutive {
                    return c;
                }
            }
            weighted_sample(&weights, k, rng)
        }
        "markov" => {
            let mut out =
                weighted_sample(&weights_from(|n| (st.freq[n as usize] + 1) as f64), 1, rng);
            while out.len() < k {
                let mut w = [0.0; 57];
                for n in 1..=56u8 {
                    if !out.contains(&n) {
                        w[n as usize] =
                            1.0 + out.iter().map(|&c| st.cooc(c, n)).sum::<u32>() as f64;
                    }
                }
                out.extend(weighted_sample(&w, 1, rng));
            }
            out.sort_unstable();
            out
        }
        _ => Vec::new(),
    }
}

/// Days since 1970-01-01 for a civil date (Howard Hinnant's algorithm).
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = (y - era * 400) as i64;
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Inverse of `days_from_civil`.
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Day of week for a day count; 0 = Sunday (1970-01-01 was a Thursday).
pub fn weekday(days: i64) -> u32 {
    (days + 4).rem_euclid(7) as u32
}

/// Parse "dd/mm/yyyy".
pub fn parse_dmy(s: &str) -> Option<(i64, u32, u32)> {
    let mut it = s.trim().split('/');
    let d = it.next()?.parse().ok()?;
    let m = it.next()?.parse().ok()?;
    let y = it.next()?.parse().ok()?;
    ((1..=31).contains(&d) && (1..=12).contains(&m)).then_some((y, m, d))
}

/// Date of the next draw: the first day after max(today, last draw) whose
/// weekday matches the drawing cadence observed in the last 30 draws.
/// Returns ("dd/mm/yyyy", weekday with 0 = Sunday).
pub fn next_draw(draws: &[Draw], today: i64) -> Option<(String, u32)> {
    let days_of = |d: &Draw| parse_dmy(&d.date).map(|(y, m, dd)| days_from_civil(y, m, dd));
    let draw_days: Vec<i64> = draws.iter().rev().take(30).filter_map(days_of).collect();
    let weekdays: Vec<u32> = draw_days.iter().map(|&d| weekday(d)).collect();
    let last = *draw_days.first()?;
    let mut day = today.max(last + 1);
    for _ in 0..14 {
        if weekdays.contains(&weekday(day)) {
            let (y, m, d) = civil_from_days(day);
            return Some((format!("{d:02}/{m:02}/{y}"), weekday(day)));
        }
        day += 1;
    }
    None
}

/// Advertised Melate+Revancha+Revanchita pot from the Lotería Nacional
/// homepage: the amount that follows the MelateRR logo in the pots carousel,
/// e.g. `Bolsas_Logo_MelateRR.svg ... Bolsa">$ 310,300,000.00`.
pub fn extract_pot(html: &str) -> Option<u64> {
    let after_logo = &html[html.find("Bolsas_Logo_MelateRR.svg")?..];
    let marker = "Bolsa\">$";
    let after_amount = &after_logo[after_logo.find(marker)? + marker.len()..];
    let raw: String = after_amount
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == ',' || *c == ' ')
        .filter(|c| c.is_ascii_digit())
        .collect();
    raw.parse().ok().filter(|&p| p > 0)
}

/// Spread `count` tickets across the selected strategies: each appears once,
/// then the remainder cycles through the non-deterministic ones only.
pub fn plan<'a>(selection: &[&'a str], count: usize) -> Vec<&'a str> {
    let mut out: Vec<&str> = selection.iter().take(count).copied().collect();
    let stochastic: Vec<&str> = selection
        .iter()
        .copied()
        .filter(|id| !DETERMINISTIC.contains(id))
        .collect();
    let mut i = 0;
    while out.len() < count && !stochastic.is_empty() {
        out.push(stochastic[i % stochastic.len()]);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::StdRng;
    use rand::SeedableRng;

    /// Hand-computed fixture. Contest 1 (no number >= 50) is pre-era; the
    /// era is contests 2, 3 and 4 (N = 3).
    const FIXTURE: &str = "\
NPRODUCTO,CONCURSO,R1,R2,R3,R4,R5,R6,R7,BOLSA,FECHA
40,1,1,2,3,4,5,6,7,0,01/01/2006
40,2,5,10,15,20,25,50,7,0,02/01/2006
40,3,5,10,30,40,45,56,7,0,03/01/2006
40,4,2,5,10,15,50,51,7,0,04/01/2006
";

    fn fixture() -> Stats {
        Stats::new(&parse(FIXTURE))
    }

    fn real() -> Stats {
        Stats::new(&parse(include_str!("../assets/Melate.csv")))
    }

    #[test]
    fn parse_filters_the_era() {
        let d = parse(FIXTURE);
        assert_eq!(d.len(), 3, "contest 1 (max 6 < 50) is not part of the era");
        assert_eq!(d[0].contest, 2);
        assert_eq!(d[2].numbers, [2, 5, 10, 15, 50, 51]);
    }

    #[test]
    fn frequencies_by_hand() {
        let st = fixture();
        // 5 and 10 appear in all 3 draws; 15 and 50 in two; 20 in one.
        assert_eq!(st.freq[5], 3);
        assert_eq!(st.freq[10], 3);
        assert_eq!(st.freq[15], 2);
        assert_eq!(st.freq[50], 2);
        assert_eq!(st.freq[20], 1);
        assert_eq!(st.freq[9], 0);
        // total: 3 draws x 6 numbers
        assert_eq!((1..=56).map(|n| st.freq[n]).sum::<u32>(), 18);
    }

    #[test]
    fn gaps_by_hand() {
        let st = fixture();
        assert_eq!(st.gap[50], 0, "appeared in the latest draw");
        assert_eq!(st.gap[30], 1, "appeared one draw before the latest");
        assert_eq!(st.gap[20], 2, "appeared only in the first era draw");
        assert_eq!(st.gap[9], 3, "never drawn: gap = N");
        assert_eq!(st.max_gap, 3);
    }

    #[test]
    fn cooccurrences_by_hand() {
        let st = fixture();
        assert_eq!(st.cooc(5, 10), 3, "5 and 10 drawn together in all 3 draws");
        assert_eq!(st.cooc(15, 50), 2, "contests 2 and 4");
        assert_eq!(st.cooc(20, 56), 0);
        assert_eq!(st.cooc(5, 10), st.cooc(10, 5), "symmetric");
        // total: N x 6 x 5 ordered pairs
        let total: u32 = (1..=56)
            .flat_map(|a| (1..=56).map(move |b| (a, b)))
            .map(|(a, b)| st.cooc(a, b))
            .sum();
        assert_eq!(total, 3 * 30);
    }

    #[test]
    fn sum_percentiles_by_hand() {
        let st = fixture();
        // sums: c2=125, c3=186, c4=133 -> sorted [125, 133, 186]
        // p15: index floor(3*0.15)=0 -> 125; p85: floor(3*0.85)=2 -> 186
        assert_eq!(st.sum_lo, 125);
        assert_eq!(st.sum_hi, 186);
    }

    #[test]
    fn exact_combinatorics() {
        assert_eq!(comb(56, 6), 32_468_436, "the Melate jackpot odds");
        assert_eq!(comb(7, 6), 7);
        assert_eq!(comb(8, 6), 28);
        assert_eq!(comb(9, 6), 84);
        assert_eq!(comb(10, 6), 210);
        assert_eq!(comb(6, 6), 1);
        assert_eq!(comb(5, 6), 0);
    }

    #[test]
    fn real_csv_invariants() {
        let st = real();
        assert!(st.n >= 2300, "the 6-of-56 era spans over 2300 draws");
        assert_eq!(
            (1..=56).map(|n| st.freq[n]).sum::<u32>(),
            6 * st.n as u32,
            "every draw contributes exactly 6 numbers"
        );
        assert_eq!((1..=56).map(|n| st.recent[n]).sum::<u32>(), 6 * WINDOW as u32);
        assert!(st.last.iter().all(|&n| st.gap[n as usize] == 0));
        assert!(st.sum_lo < st.sum_hi);
        assert!((100..=250).contains(&st.sum_lo), "p15 sum in a sane range");
        assert_eq!(st.f_den(), 6 * st.n as u32 + 56);
    }

    #[test]
    fn hot_picks_the_most_frequent() {
        let st = real();
        let mut rng = StdRng::seed_from_u64(1);
        let c = generate(&st, "hot", 6, &mut rng);
        let worst_inside = c.iter().map(|&n| st.freq[n as usize]).min().unwrap();
        let best_outside = (1..=56u8)
            .filter(|n| !c.contains(n))
            .map(|n| st.freq[n as usize])
            .max()
            .unwrap();
        assert!(worst_inside >= best_outside);
    }

    #[test]
    fn every_strategy_yields_k_valid_numbers() {
        let st = real();
        let mut rng = StdRng::seed_from_u64(2);
        for id in STRATEGIES {
            for k in [6, 7, 10] {
                let c = generate(&st, id, k, &mut rng);
                assert_eq!(c.len(), k, "{id} with k={k}");
                assert!(c.windows(2).all(|w| w[0] < w[1]), "{id}: sorted, no repeats");
                assert!(c.iter().all(|&n| (1..=56).contains(&n)), "{id}: range");
            }
        }
    }

    #[test]
    fn balanced_honors_its_constraints() {
        let st = real();
        let mut rng = StdRng::seed_from_u64(3);
        for _ in 0..50 {
            let c = generate(&st, "balanced", 6, &mut rng);
            let sum: u32 = c.iter().map(|&n| n as u32).sum();
            let evens = c.iter().filter(|&&n| n % 2 == 0).count();
            let lows = c.iter().filter(|&&n| n <= 28).count();
            assert!((st.sum_lo..=st.sum_hi).contains(&sum));
            assert!((2..=4).contains(&evens));
            assert!((2..=4).contains(&lows));
        }
    }

    #[test]
    fn sterncover_avoids_popular_shapes() {
        let st = real();
        let mut rng = StdRng::seed_from_u64(4);
        for _ in 0..50 {
            let c = generate(&st, "sterncover", 6, &mut rng);
            assert!(c.iter().filter(|&&n| n >= 32).count() >= 4);
            assert!(!c.windows(2).any(|w| w[1] - w[0] == 1), "no consecutive numbers");
        }
    }

    #[test]
    fn gamma_has_the_right_mean() {
        let mut rng = StdRng::seed_from_u64(5);
        let samples = 3000;
        let mean: f64 =
            (0..samples).map(|_| gamma_int(300, &mut rng)).sum::<f64>() / samples as f64;
        assert!((mean - 300.0).abs() < 5.0, "E[Gamma(300,1)] = 300, got {mean}");
    }

    #[test]
    fn weighted_sampling_respects_weights() {
        // A number holding all the weight always wins; near-zero never does.
        let mut w = [0.0f64; 57];
        w[7] = 1.0;
        w[13] = 1e-12;
        let mut rng = StdRng::seed_from_u64(6);
        for _ in 0..100 {
            assert_eq!(weighted_sample(&w, 1, &mut rng), vec![7]);
        }
    }

    #[test]
    fn plan_spreads_tickets_correctly() {
        let sel = ["hot", "cold", "weighted", "markov"];
        let p = plan(&sel, 8);
        assert_eq!(p.len(), 8);
        assert_eq!(p.iter().filter(|&&s| s == "hot").count(), 1, "deterministic: once");
        assert_eq!(p.iter().filter(|&&s| s == "cold").count(), 1);
        // only deterministic strategies selected: nothing to fill with
        assert_eq!(plan(&["hot", "cold"], 8).len(), 2);
        assert_eq!(plan(&sel, 2), vec!["hot", "cold"]);
    }

    #[test]
    fn civil_days_anchors_and_roundtrip() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(weekday(days_from_civil(1970, 1, 1)), 4, "1970-01-01 was a Thursday");
        assert_eq!(weekday(days_from_civil(2026, 9, 20)), 0, "2026-09-20 was a Sunday");
        for &(y, m, d) in &[(2026, 9, 20), (2000, 2, 29), (1984, 8, 19), (2100, 12, 31)] {
            assert_eq!(civil_from_days(days_from_civil(y, m, d)), (y, m, d));
        }
    }

    #[test]
    fn dmy_parsing() {
        assert_eq!(parse_dmy("20/09/2026"), Some((2026, 9, 20)));
        assert_eq!(parse_dmy(" 02/01/2006 "), Some((2006, 1, 2)));
        assert_eq!(parse_dmy("2026-09-20"), None);
        assert_eq!(parse_dmy("40/01/2026"), None);
    }

    #[test]
    fn next_draw_follows_the_cadence() {
        // Fixture era draws fall on 02..04/01/2006 = Mon, Tue, Wed.
        let draws = parse(FIXTURE);
        let today = days_from_civil(2006, 1, 4);
        let (date, wd) = next_draw(&draws, today).unwrap();
        assert_eq!(date, "09/01/2006", "next Monday after Wed Jan 4");
        assert_eq!(wd, 1, "Monday");
        // If the CSV is stale, the next draw is never in the past.
        let later = days_from_civil(2006, 3, 15); // a Wednesday
        let (date2, wd2) = next_draw(&draws, later).unwrap();
        assert_eq!(date2, "15/03/2006", "that same Wednesday");
        assert_eq!(wd2, 3);
    }

    #[test]
    fn advertised_pot_is_extracted_from_homepage_html() {
        let html = r#"<img src="x/Bolsas_Logo_Zodiaco.svg"><div class="Bolsa">$ 24,042,600.00</div>
<img src="x/Bolsas_Logo_MelateRR.svg"><div class="Bolsa">$ 310,300,000.00</div>"#;
        assert_eq!(extract_pot(html), Some(310_300_000));
        assert_eq!(extract_pot("no carousel here"), None);
        assert_eq!(extract_pot(r#"Bolsas_Logo_MelateRR.svg but no amount"#), None);
    }

    #[test]
    fn pot_is_parsed() {
        let st = fixture();
        assert_eq!(st.last_pot, 0, "fixture pots are 0");
        let real = real();
        assert!(real.last_pot > 1_000_000, "real pot is in the millions");
    }

    #[test]
    fn best_pair_matches_cooc() {
        let st = real();
        let (m, v) = st.best_pair(8);
        assert_eq!(st.cooc(8, m), v);
        assert!((1..=56u8).filter(|&x| x != 8).all(|x| st.cooc(8, x) <= v));
    }
}
