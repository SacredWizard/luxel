//! Generic level-of-detail pyramid (an implicit, append-friendly segment tree).
//!
//! Level 0 is the raw series. Node `i` of level `k + 1` is the ordered merge of
//! nodes `2i` and `2i + 1` of level `k` (or just `2i` when it is the last, odd
//! node). Memory overhead is < 1x the base series.

/// Values that can be combined in order. `merge` must be associative:
/// `a.merge(b).merge(c) == a.merge(&b.merge(c))`.
pub trait Aggregate: Copy {
    /// Combine `self` (earlier) with `next` (later).
    fn merge(&self, next: &Self) -> Self;
}

/// Min/max envelope; the aggregate behind M4-style line downsampling that
/// never drops a spike. NaN inputs are ignored by `merge` (`f64::min/max`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MinMax {
    pub min: f64,
    pub max: f64,
}

impl MinMax {
    pub fn point(v: f64) -> Self {
        Self { min: v, max: v }
    }
}

impl Aggregate for MinMax {
    #[inline]
    fn merge(&self, next: &Self) -> Self {
        Self {
            min: self.min.min(next.min),
            max: self.max.max(next.max),
        }
    }
}

impl Aggregate for f64 {
    /// Plain sum (histograms, volume).
    #[inline]
    fn merge(&self, next: &Self) -> Self {
        self + next
    }
}

#[derive(Clone, Debug)]
pub struct Pyramid<T: Aggregate> {
    levels: Vec<Vec<T>>,
}

impl<T: Aggregate> Default for Pyramid<T> {
    fn default() -> Self {
        Self {
            levels: vec![Vec::new()],
        }
    }
}

impl<T: Aggregate> Pyramid<T> {
    /// Build all levels in `O(n)`.
    pub fn build(base: Vec<T>) -> Self {
        let mut levels = vec![base];
        while levels.last().unwrap().len() > 1 {
            let prev = levels.last().unwrap();
            let next: Vec<T> = prev
                .chunks(2)
                .map(|c| {
                    if c.len() == 2 {
                        c[0].merge(&c[1])
                    } else {
                        c[0]
                    }
                })
                .collect();
            levels.push(next);
        }
        Self { levels }
    }

    pub fn base(&self) -> &[T] {
        &self.levels[0]
    }

    pub fn len(&self) -> usize {
        self.levels[0].len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn levels(&self) -> &[Vec<T>] {
        &self.levels
    }

    /// Level `k`; each element covers `2^k` base items.
    pub fn level(&self, k: usize) -> &[T] {
        &self.levels[k.min(self.levels.len() - 1)]
    }

    pub fn max_level(&self) -> usize {
        self.levels.len() - 1
    }

    /// Append one item (streaming data). `O(log n)`.
    pub fn push(&mut self, item: T) {
        self.levels[0].push(item);
        self.fix_tail();
    }

    /// Replace the last item (e.g. the still-forming candle). `O(log n)`.
    pub fn update_last(&mut self, item: T) {
        if let Some(last) = self.levels[0].last_mut() {
            *last = item;
            self.fix_tail();
        } else {
            self.push(item);
        }
    }

    fn fix_tail(&mut self) {
        let mut k = 0;
        while self.levels[k].len() > 1 {
            let idx = (self.levels[k].len() - 1) / 2;
            let child = &self.levels[k];
            let node = match child.get(2 * idx + 1) {
                Some(r) => child[2 * idx].merge(r),
                None => child[2 * idx],
            };
            if k + 1 == self.levels.len() {
                self.levels.push(Vec::new());
            }
            let parent = &mut self.levels[k + 1];
            if idx < parent.len() {
                parent[idx] = node;
            } else {
                parent.push(node);
            }
            k += 1;
        }
        // drop stale top levels (never happens on append, kept for safety)
        self.levels.truncate(k + 1);
    }

    /// Ordered aggregate of base items `[start, end)`. `O(log n)`.
    pub fn query(&self, start: usize, end: usize) -> Option<T> {
        let end = end.min(self.len());
        if start >= end {
            return None;
        }
        let (mut s, mut e, mut k) = (start, end, 0);
        let mut left: Option<T> = None;
        let mut right: Option<T> = None;
        while s < e {
            let lvl = &self.levels[k];
            if s & 1 == 1 {
                left = Some(match left {
                    Some(l) => l.merge(&lvl[s]),
                    None => lvl[s],
                });
                s += 1;
            }
            if e & 1 == 1 {
                e -= 1;
                right = Some(match right {
                    Some(r) => lvl[e].merge(&r),
                    None => lvl[e],
                });
            }
            s /= 2;
            e /= 2;
            k += 1;
        }
        match (left, right) {
            (Some(l), Some(r)) => Some(l.merge(&r)),
            (l, r) => l.or(r),
        }
    }

    /// Items of level `k` overlapping base range `[start, end)`, plus the
    /// base index of the first returned item. This is "what to draw".
    pub fn visible(&self, k: usize, start: usize, end: usize) -> (usize, &[T]) {
        let k = k.min(self.max_level());
        let lvl = &self.levels[k];
        let s = (start >> k).min(lvl.len());
        let e = (end.saturating_add((1 << k) - 1) >> k)
            .min(lvl.len())
            .max(s);
        (s << k, &lvl[s..e])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ordered "concat" aggregate to check that merge order is preserved.
    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Span(usize, usize);
    impl Aggregate for Span {
        fn merge(&self, n: &Self) -> Self {
            assert_eq!(self.1, n.0, "merge out of order");
            Span(self.0, n.1)
        }
    }

    #[test]
    fn query_is_ordered_and_exact() {
        for n in [1usize, 2, 3, 7, 64, 100, 1025] {
            let p = Pyramid::build((0..n).map(|i| Span(i, i + 1)).collect());
            for s in 0..n.min(40) {
                for e in s + 1..=n {
                    assert_eq!(p.query(s, e), Some(Span(s, e)), "n={n} {s}..{e}");
                }
            }
        }
    }

    #[test]
    fn push_matches_build() {
        let data: Vec<MinMax> = (0..777)
            .map(|i| MinMax::point(((i * 37) % 101) as f64))
            .collect();
        let mut p = Pyramid::build(data[..10].to_vec());
        for d in &data[10..] {
            p.push(*d);
        }
        p.update_last(MinMax::point(1e9));
        let mut expect = data.clone();
        *expect.last_mut().unwrap() = MinMax::point(1e9);
        let q = Pyramid::build(expect);
        assert_eq!(p.levels(), q.levels());
        assert_eq!(p.query(0, 777).unwrap().max, 1e9);
    }

    #[test]
    fn visible_covers_range() {
        let p = Pyramid::build((0..1000).map(|i| Span(i, i + 1)).collect());
        let (first, items) = p.visible(3, 17, 101);
        assert_eq!(first, 16);
        assert_eq!(items.first().unwrap().0, 16);
        assert!(items.last().unwrap().1 >= 101);
    }
}
