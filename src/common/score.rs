//! The arithmetic that mixes two search indexes.
//!
//! The memory and the code index both ask a keyword index and a vector index,
//! and both must turn two answers into one order. The numbers that the two
//! indexes give cannot be compared as they arrive: bm25 gives a negative
//! number on a scale that changes with the question, and the vector index
//! gives a distance. The functions here put both on the same scale and cut
//! the answers that are too far away.
//!
//! Nothing here knows what a fact or a node is. Each function takes the
//! scored items of the caller and gives them back.

/// How many items one index gives back before the mix.
///
/// It is more than the caller wants, because the mix with the other index
/// changes the order: an item that sits below the limit in one index can
/// reach the top after the two are added.
pub fn candidate_count(limit: usize) -> i64 {
    (limit * 4).max(50) as i64
}

/// Turns the distance of the vector index into a score.
///
/// The vectors have a length of one, and the index measures the straight
/// distance `d` between two of them. For such vectors the angle gives
/// `cos = 1 - d² / 2`, which is 1.0 for two texts that say the same, 0.0 for
/// two texts with nothing in common, and -1.0 for two texts that say the
/// opposite.
///
/// This is an absolute scale, so it needs no other hit to make sense of it.
/// The keyword index has no such scale, which is why [`rescale`] exists for
/// that one alone.
pub fn similarity(distance: f64) -> f64 {
    1.0 - (distance * distance) / 2.0
}

/// Turns the rank of the keyword index into a number from 0.0 to 1.0.
///
/// bm25 gives a negative number, and the best match is the smallest. The
/// number says nothing on its own: it depends on the words of the question
/// and on the whole collection. This therefore maps the best hit of the list
/// to 1.0 and the worst to 0.0.
///
/// A list of one hit, or a list where every hit ties, scores 1.0 throughout:
/// with no spread there is nothing to tell the hits apart.
pub fn rescale<T>(hits: Vec<(T, f64)>) -> Vec<(T, f64)> {
    let best = hits.iter().map(|(_, rank)| *rank).fold(f64::MAX, f64::min);
    let worst = hits.iter().map(|(_, rank)| *rank).fold(f64::MIN, f64::max);
    let spread = (worst - best).abs();

    hits.into_iter()
        .map(|(item, rank)| {
            let score = if spread < f64::EPSILON {
                1.0
            } else {
                (worst - rank) / spread
            };
            (item, score)
        })
        .collect()
}

/// Drops the hits of the vector index that are too far away.
///
/// The index answers with the nearest items even when none of them is near.
/// Without a cut, a question that the collection cannot answer would still
/// come back full.
///
/// The two numbers work together. `floor` is one value for every question and
/// drops a whole answer that is bad. `share` reads the answer that came back
/// and keeps the items that come near the best of them, which drops the tail
/// of a good answer.
pub fn cut<T>(hits: &mut Vec<(T, f64)>, floor: f64, share: f64) {
    let best = hits
        .iter()
        .map(|(_, score)| *score)
        .fold(f64::MIN, f64::max);
    let cut = floor.max(best * share);
    hits.retain(|(_, score)| *score >= cut);
}

/// How many items a word may reach before it says nothing.
///
/// A word such as "the" reaches almost every item of a collection. It
/// therefore tells no item from another, and a question that holds it would
/// drag the whole collection into the answer. A word that reaches more items
/// than this leaves the expression.
pub fn ceiling(total: i64, share: f64) -> i64 {
    (total as f64 * share).floor() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_index_gives_back_more_than_the_caller_wants() {
        assert_eq!(candidate_count(20), 80);
        // A small limit still asks for enough to make the mix mean something.
        assert_eq!(candidate_count(1), 50);
    }

    #[test]
    fn the_distance_becomes_the_angle() {
        // Two vectors in the same place say the same thing.
        assert!((similarity(0.0) - 1.0).abs() < f64::EPSILON);
        // Two unit vectors at a right angle sit at a distance of sqrt(2).
        assert!(similarity(2.0f64.sqrt()).abs() < 1e-12);
        // Two unit vectors that point apart sit at a distance of 2.
        assert!((similarity(2.0) + 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn the_best_keyword_hit_becomes_one_and_the_worst_becomes_zero() {
        let scored = rescale(vec![("a", -5.0), ("b", -3.0), ("c", -1.0)]);
        assert_eq!(scored[0], ("a", 1.0));
        assert_eq!(scored[2], ("c", 0.0));
        assert!(scored[1].1 > 0.0 && scored[1].1 < 1.0);
    }

    #[test]
    fn a_list_with_no_spread_scores_one_throughout() {
        let scored = rescale(vec![("a", -2.0), ("b", -2.0)]);
        assert_eq!(scored, vec![("a", 1.0), ("b", 1.0)]);
        assert_eq!(rescale(vec![("only", -7.0)]), vec![("only", 1.0)]);
    }

    #[test]
    fn an_empty_list_survives_the_rescale() {
        assert!(rescale(Vec::<(&str, f64)>::new()).is_empty());
    }

    #[test]
    fn the_floor_drops_a_whole_answer_that_is_bad() {
        let mut hits = vec![("a", 0.10), ("b", 0.05)];
        cut(&mut hits, 0.15, 0.0);
        assert!(hits.is_empty());
    }

    #[test]
    fn the_share_drops_the_tail_of_a_good_answer() {
        let mut hits = vec![("a", 0.90), ("b", 0.50), ("c", 0.20)];
        cut(&mut hits, 0.0, 0.5);
        // The cut sits at 0.45, so only the tail goes.
        assert_eq!(hits, vec![("a", 0.90), ("b", 0.50)]);
    }

    #[test]
    fn the_higher_of_the_two_cuts_wins() {
        let mut hits = vec![("a", 0.90), ("b", 0.50)];
        cut(&mut hits, 0.60, 0.5);
        assert_eq!(hits, vec![("a", 0.90)]);
    }

    #[test]
    fn a_word_that_reaches_half_the_collection_sits_at_the_ceiling() {
        assert_eq!(ceiling(100, 0.5), 50);
        // A share of one or above keeps every word.
        assert_eq!(ceiling(100, 1.0), 100);
        assert_eq!(ceiling(0, 0.5), 0);
    }
}
