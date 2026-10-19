//! The learning itself: TF-IDF over character n-grams of each word, and a multinomial
//! logistic regression fitted with L-BFGS: the equivalent of scikit-learn's
//! `TfidfVectorizer(analyzer="char_wb", ngram_range=(2, 5), sublinear_tf=True)` and
//! `LogisticRegression(C=20)`.

use std::collections::HashMap;

/// A document's features: (feature index, value), sorted by index.
pub type Row = Vec<(usize, f64)>;

const MIN_N: usize = 2;
const MAX_N: usize = 5;

/// The n-grams of each word, padded with a space either side, as scikit-learn's `char_wb`
/// analyser makes them (after lower-casing).
fn char_wb_ngrams(text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    let mut grams = Vec::new();
    for word in lower.split_whitespace() {
        let padded: Vec<char> = format!(" {word} ").chars().collect();
        for n in MIN_N..=MAX_N {
            let mut offset = 0;
            grams.push(
                padded[offset..(offset + n).min(padded.len())]
                    .iter()
                    .collect(),
            );
            while offset + n < padded.len() {
                offset += 1;
                grams.push(padded[offset..offset + n].iter().collect());
            }
            if offset == 0 {
                // The word is shorter than n: its one n-gram is the whole padded word.
                break;
            }
        }
    }
    grams
}

/// Term frequency-inverse document frequency: sublinear counts, smoothed idf, rows scaled to
/// unit length.
#[derive(Debug, Clone)]
pub struct Vectoriser {
    vocabulary: HashMap<String, usize>,
    idf: Vec<f64>,
}

impl Vectoriser {
    pub fn fit(documents: &[String]) -> (Self, Vec<Row>) {
        let mut vocabulary: HashMap<String, usize> = HashMap::new();
        let mut counts: Vec<HashMap<usize, f64>> = Vec::with_capacity(documents.len());
        for document in documents {
            let mut doc_counts: HashMap<usize, f64> = HashMap::new();
            for gram in char_wb_ngrams(document) {
                let next = vocabulary.len();
                let index = *vocabulary.entry(gram).or_insert(next);
                *doc_counts.entry(index).or_default() += 1.0;
            }
            counts.push(doc_counts);
        }
        let mut document_frequency = vec![0.0; vocabulary.len()];
        for doc in &counts {
            for index in doc.keys() {
                document_frequency[*index] += 1.0;
            }
        }
        let n = documents.len() as f64;
        let idf = document_frequency
            .iter()
            .map(|df| ((1.0 + n) / (1.0 + df)).ln() + 1.0)
            .collect();
        let vectoriser = Self { vocabulary, idf };
        let rows = counts.into_iter().map(|c| vectoriser.weigh(c)).collect();
        (vectoriser, rows)
    }

    /// A new document's features. N-grams the training data never had are ignored.
    pub fn transform(&self, document: &str) -> Row {
        let mut counts: HashMap<usize, f64> = HashMap::new();
        for gram in char_wb_ngrams(document) {
            if let Some(index) = self.vocabulary.get(&gram) {
                *counts.entry(*index).or_default() += 1.0;
            }
        }
        self.weigh(counts)
    }

    pub fn features(&self) -> usize {
        self.idf.len()
    }

    fn weigh(&self, counts: HashMap<usize, f64>) -> Row {
        let mut row: Row = counts
            .into_iter()
            .map(|(index, count)| (index, (1.0 + count.ln()) * self.idf[index]))
            .collect();
        row.sort_unstable_by_key(|(index, _)| *index);
        let norm = row.iter().map(|(_, v)| v * v).sum::<f64>().sqrt();
        if norm > 0.0 {
            for (_, value) in &mut row {
                *value /= norm;
            }
        }
        row
    }
}

/// Multinomial logistic regression with an L2 penalty of strength 1/C on the weights (not the
/// intercepts).
#[derive(Debug, Clone)]
pub struct Classifier {
    /// The label each class stands for, in index order.
    pub classes: Vec<i64>,
    features: usize,
    /// Class-major: class k's weights are `weights[k * features..(k + 1) * features]`.
    weights: Vec<f64>,
    intercepts: Vec<f64>,
}

impl Classifier {
    pub fn fit(rows: &[Row], labels: &[i64], features: usize, c: f64) -> Self {
        let mut classes: Vec<i64> = labels.to_vec();
        classes.sort_unstable();
        classes.dedup();
        let index: HashMap<i64, usize> = classes.iter().enumerate().map(|(i, c)| (*c, i)).collect();
        let targets: Vec<usize> = labels.iter().map(|l| index[l]).collect();
        let k = classes.len();
        let problem = Problem {
            rows,
            targets: &targets,
            classes: k,
            features,
            penalty: 1.0 / c,
        };
        let parameters = lbfgs(&problem, vec![0.0; k * features + k]);
        let (weights, intercepts) = parameters.split_at(k * features);
        Self {
            classes,
            features,
            weights: weights.to_vec(),
            intercepts: intercepts.to_vec(),
        }
    }

    /// The probability of each class, in `classes` order.
    pub fn probabilities(&self, row: &Row) -> Vec<f64> {
        let scores: Vec<f64> = (0..self.classes.len())
            .map(|k| {
                let weights = &self.weights[k * self.features..(k + 1) * self.features];
                self.intercepts[k] + row.iter().map(|(i, v)| weights[*i] * v).sum::<f64>()
            })
            .collect();
        softmax(&scores)
    }

    /// The most likely label and its probability.
    pub fn predict(&self, row: &Row) -> (i64, f64) {
        let probabilities = self.probabilities(row);
        let (best, p) = probabilities
            .iter()
            .enumerate()
            .fold(
                (0, f64::MIN),
                |best, (i, p)| if *p > best.1 { (i, *p) } else { best },
            );
        (self.classes[best], p)
    }
}

fn softmax(scores: &[f64]) -> Vec<f64> {
    let max = scores.iter().copied().fold(f64::MIN, f64::max);
    let exps: Vec<f64> = scores.iter().map(|s| (s - max).exp()).collect();
    let total: f64 = exps.iter().sum();
    exps.into_iter().map(|e| e / total).collect()
}

struct Problem<'a> {
    rows: &'a [Row],
    targets: &'a [usize],
    classes: usize,
    features: usize,
    penalty: f64,
}

impl Problem<'_> {
    /// The loss (cross-entropy summed over rows, plus the penalty) and its gradient.
    fn evaluate(&self, parameters: &[f64]) -> (f64, Vec<f64>) {
        let (k, f) = (self.classes, self.features);
        let (weights, intercepts) = parameters.split_at(k * f);
        let mut gradient = vec![0.0; parameters.len()];
        let mut loss = 0.0;
        let mut scores = vec![0.0; k];
        for (row, target) in self.rows.iter().zip(self.targets) {
            for (class, score) in scores.iter_mut().enumerate() {
                let w = &weights[class * f..(class + 1) * f];
                *score = intercepts[class] + row.iter().map(|(i, v)| w[*i] * v).sum::<f64>();
            }
            let max = scores.iter().copied().fold(f64::MIN, f64::max);
            let total: f64 = scores.iter().map(|s| (s - max).exp()).sum();
            loss += max + total.ln() - scores[*target];
            for (class, score) in scores.iter().enumerate() {
                let p = (score - max).exp() / total;
                let error = p - if class == *target { 1.0 } else { 0.0 };
                let g = &mut gradient[class * f..(class + 1) * f];
                for (i, v) in row {
                    g[*i] += error * v;
                }
                gradient[k * f + class] += error;
            }
        }
        for (i, w) in weights.iter().enumerate() {
            loss += 0.5 * self.penalty * w * w;
            gradient[i] += self.penalty * w;
        }
        (loss, gradient)
    }
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Limited-memory BFGS with a backtracking line search, until the gradient is near zero.
fn lbfgs(problem: &Problem<'_>, start: Vec<f64>) -> Vec<f64> {
    const MEMORY: usize = 10;
    const MAX_ITERATIONS: usize = 5000;
    const GRADIENT_TOLERANCE: f64 = 1e-5;

    let mut x = start;
    let (mut loss, mut gradient) = problem.evaluate(&x);
    let mut history: std::collections::VecDeque<(Vec<f64>, Vec<f64>, f64)> =
        std::collections::VecDeque::new();

    for _ in 0..MAX_ITERATIONS {
        let largest = gradient.iter().fold(0.0_f64, |m, g| m.max(g.abs()));
        // The loss is summed over rows; scikit-learn stops at 1e-4 on the mean.
        if largest <= GRADIENT_TOLERANCE * problem.rows.len().max(1) as f64 {
            break;
        }
        // Two-loop recursion: the search direction from recent steps.
        let mut direction: Vec<f64> = gradient.iter().map(|g| -g).collect();
        let mut alphas = Vec::with_capacity(history.len());
        for (s, y, rho) in history.iter().rev() {
            let alpha = rho * dot(s, &direction);
            for (d, yi) in direction.iter_mut().zip(y) {
                *d -= alpha * yi;
            }
            alphas.push(alpha);
        }
        if let Some((s, y, _)) = history.back() {
            let scale = dot(s, y) / dot(y, y);
            for d in &mut direction {
                *d *= scale;
            }
        } else {
            let norm = dot(&gradient, &gradient).sqrt();
            for d in &mut direction {
                *d /= norm.max(1.0);
            }
        }
        for ((s, y, rho), alpha) in history.iter().zip(alphas.iter().rev()) {
            let beta = rho * dot(y, &direction);
            for (d, si) in direction.iter_mut().zip(s) {
                *d += (alpha - beta) * si;
            }
        }
        let mut slope = dot(&gradient, &direction);
        if slope >= 0.0 {
            // Not a descent direction: start again from steepest descent.
            history.clear();
            direction = gradient.iter().map(|g| -g).collect();
            slope = dot(&gradient, &direction);
        }

        // Backtracking line search (Armijo condition).
        let mut step = 1.0;
        let (next, next_loss, next_gradient) = loop {
            let candidate: Vec<f64> = x
                .iter()
                .zip(&direction)
                .map(|(xi, d)| xi + step * d)
                .collect();
            let (candidate_loss, candidate_gradient) = problem.evaluate(&candidate);
            if candidate_loss <= loss + 1e-4 * step * slope || step < 1e-12 {
                break (candidate, candidate_loss, candidate_gradient);
            }
            step *= 0.5;
        };
        let s: Vec<f64> = next.iter().zip(&x).map(|(a, b)| a - b).collect();
        let y: Vec<f64> = next_gradient
            .iter()
            .zip(&gradient)
            .map(|(a, b)| a - b)
            .collect();
        let sy = dot(&s, &y);
        let improvement = loss - next_loss;
        x = next;
        loss = next_loss;
        gradient = next_gradient;
        if sy > 1e-12 {
            if history.len() == MEMORY {
                history.pop_front();
            }
            history.push_back((s, y, 1.0 / sy));
        }
        if improvement.abs() <= 1e-12 * loss.abs().max(1.0) {
            break;
        }
    }
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ngrams_follow_scikit_learns_char_wb() {
        assert_eq!(
            char_wb_ngrams("AB"),
            [" a", "ab", "b ", " ab", "ab ", " ab "]
        );
        // A one-letter word gives one n-gram per length up to its padded length.
        assert_eq!(char_wb_ngrams("x"), [" x", "x ", " x "]);
    }

    #[test]
    fn rows_are_unit_length_and_unknown_grams_are_ignored() {
        let docs = vec!["tesco stores".to_owned(), "netflix".to_owned()];
        let (vectoriser, rows) = Vectoriser::fit(&docs);
        for row in &rows {
            let norm: f64 = row.iter().map(|(_, v)| v * v).sum();
            assert!((norm - 1.0).abs() < 1e-9);
        }
        assert!(vectoriser.transform("zzzz").is_empty());
        assert_eq!(vectoriser.transform("netflix"), rows[1]);
    }

    #[test]
    fn idf_is_smoothed_like_scikit_learns() {
        // Two documents; " a" is in both, so idf = ln(3/3) + 1 = 1; "zz" in one: ln(3/2) + 1.
        let (vectoriser, _) = Vectoriser::fit(&["a".to_owned(), "a zz".to_owned()]);
        let idf = |gram: &str| vectoriser.idf[vectoriser.vocabulary[gram]];
        assert!((idf(" a") - 1.0).abs() < 1e-12);
        assert!((idf("zz") - (1.5_f64.ln() + 1.0)).abs() < 1e-12);
    }

    #[test]
    fn learns_to_tell_classes_apart() {
        let docs: Vec<String> = [
            "tesco stores",
            "tesco express",
            "shell petrol",
            "esso petrol",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
        let labels = [1, 1, 2, 2];
        let (vectoriser, rows) = Vectoriser::fit(&docs);
        let classifier = Classifier::fit(&rows, &labels, vectoriser.features(), 20.0);
        let (label, p) = classifier.predict(&vectoriser.transform("tesco metro"));
        assert_eq!(label, 1);
        assert!(p > 0.5);
        let (label, _) = classifier.predict(&vectoriser.transform("bp petrol"));
        assert_eq!(label, 2);
        let total: f64 = classifier.probabilities(&rows[0]).iter().sum();
        assert!((total - 1.0).abs() < 1e-9);
    }
}
