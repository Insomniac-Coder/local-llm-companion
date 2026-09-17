//! The owner's speed rule: how the app chooses between runtime options that
//! trade generation speed against prompt reading speed (micro-batch size, load
//! mode, drafting).
//!
//! Owner rule (2026-09-16 night; replaces "highest output speed"): aim for a
//! really good balance of generation and prompt reading. Start from the option
//! with the fastest generation. Switch to the option that reads prompts fastest
//! among those that give up **at most 5% of generation** and gain **at least 5x
//! as much prompt reading (in %) as the generation they lose**.
//!
//! Measured example (a 30B mixture-of-experts model with experts partly in RAM,
//! 4,096-token prompt, each micro-batch with its own fit): micro-batch 1,024 is
//! chosen over 512 (-2% generation, +46% prompt reading); 2,048 is not (-11%
//! generation). An option that gains generation (a built-in draft head: +54%
//! generation, -5.7% prompt reading (795 -> 750)) wins at the first step anyway.
//!
//! Pure and unit-tested.

/// Measured speeds of one option, in tokens per second (higher is faster).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Speed {
    pub generation_tps: f64,
    pub prompt_tps: f64,
}

/// The most generation an option may give up, as a share of the fastest.
pub const MAX_GENERATION_LOSS: f64 = 0.05;
/// Prompt reading an option must gain per unit of generation it loses (both
/// relative to the fastest generation).
pub const MIN_GAIN_PER_LOSS: f64 = 5.0;
/// Rounding slack, so a value exactly on a limit counts as within it.
const TOLERANCE: f64 = 1e-9;

/// The index of the option the rule chooses, or None when no option has a
/// usable measurement. Options with a non-finite or non-positive speed are
/// skipped.
///
/// The base is the fastest generation (on a tie the faster prompt reading,
/// then the earlier option). Another option replaces it when it loses at most
/// `MAX_GENERATION_LOSS` of the base's generation, reads prompts faster, and
/// gains at least `MIN_GAIN_PER_LOSS` times the share it loses; of those, the
/// fastest prompt reading wins (on a tie the faster generation, then the
/// earlier option).
pub fn balanced_choice(options: &[Speed]) -> Option<usize> {
    let usable = |speed: &Speed| {
        speed.generation_tps.is_finite()
            && speed.prompt_tps.is_finite()
            && speed.generation_tps > 0.0
            && speed.prompt_tps > 0.0
    };
    let mut base: Option<usize> = None;
    for (index, option) in options.iter().enumerate() {
        if !usable(option) {
            continue;
        }
        base = match base {
            Some(best)
                if option.generation_tps < options[best].generation_tps
                    || (option.generation_tps == options[best].generation_tps
                        && option.prompt_tps <= options[best].prompt_tps) =>
            {
                Some(best)
            }
            _ => Some(index),
        };
    }
    let base = base?;
    let reference = options[base];
    let mut chosen = base;
    for (index, option) in options.iter().enumerate() {
        if index == base || !usable(option) {
            continue;
        }
        let loss = 1.0 - option.generation_tps / reference.generation_tps;
        let gain = option.prompt_tps / reference.prompt_tps - 1.0;
        if loss > MAX_GENERATION_LOSS + TOLERANCE || gain <= 0.0 || gain + TOLERANCE < MIN_GAIN_PER_LOSS * loss.max(0.0) {
            continue;
        }
        let current = options[chosen];
        if chosen == base
            || option.prompt_tps > current.prompt_tps
            || (option.prompt_tps == current.prompt_tps && option.generation_tps > current.generation_tps)
        {
            chosen = index;
        }
    }
    Some(chosen)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn speed(generation_tps: f64, prompt_tps: f64) -> Speed {
        Speed { generation_tps, prompt_tps }
    }

    #[test]
    fn the_measured_micro_batch_table_picks_1024() {
        // 30B mixture-of-experts model, 4,096-token prompt, each micro-batch
        // with its own fit: 512, 1,024, 2,048, 4,096.
        let table = [speed(67.5, 1_617.0), speed(66.1, 2_369.0), speed(59.8, 3_091.0), speed(53.5, 3_453.0)];
        assert_eq!(balanced_choice(&table), Some(1), "-2% generation for +46% prompt reading");
        let reversed = [table[3], table[2], table[1], table[0]];
        assert_eq!(balanced_choice(&reversed), Some(2), "the order of the options does not matter");
    }

    #[test]
    fn micro_batch_2048_loses_too_much_generation() {
        // -11% generation, far over the 5% limit, however large the prompt gain.
        assert_eq!(balanced_choice(&[speed(67.5, 1_617.0), speed(59.8, 3_091.0)]), Some(0));
    }

    #[test]
    fn loading_without_mmap_beats_mmap() {
        // 2,048-token prompt; mmap first, load mode none second, both passes.
        assert_eq!(balanced_choice(&[speed(55.2, 2_079.0), speed(60.9, 3_126.0)]), Some(1));
        assert_eq!(balanced_choice(&[speed(59.3, 2_160.0), speed(59.3, 3_179.0)]), Some(1), "equal generation: faster prompt reading is the base");
    }

    #[test]
    fn a_draft_head_wins_on_generation_despite_slower_prompts() {
        // 27B: drafting off, then the built-in draft head.
        assert_eq!(balanced_choice(&[speed(34.9, 795.0), speed(53.7, 750.0)]), Some(1));
    }

    #[test]
    fn a_loss_over_five_percent_is_rejected_even_for_a_huge_gain() {
        // -5.7% generation for +200% prompt reading.
        assert_eq!(balanced_choice(&[speed(100.0, 1_000.0), speed(94.3, 3_000.0)]), Some(0));
    }

    #[test]
    fn a_gain_under_five_times_the_loss_is_rejected() {
        // -2% generation for +8% prompt reading: a ratio of 4.
        assert_eq!(balanced_choice(&[speed(100.0, 1_000.0), speed(98.0, 1_080.0)]), Some(0));
        // Exactly on both limits: -5% for +25%.
        assert_eq!(balanced_choice(&[speed(100.0, 1_000.0), speed(95.0, 1_250.0)]), Some(1));
    }

    #[test]
    fn the_fastest_prompt_reader_among_qualifying_options_wins() {
        let options = [speed(100.0, 1_000.0), speed(99.0, 1_100.0), speed(97.0, 1_300.0), speed(96.0, 1_200.0)];
        assert_eq!(balanced_choice(&options), Some(2));
        let tied = [speed(100.0, 1_000.0), speed(98.0, 1_300.0), speed(99.0, 1_300.0)];
        assert_eq!(balanced_choice(&tied), Some(2), "equal prompt reading: the faster generation");
        assert_eq!(balanced_choice(&[speed(60.0, 1_000.0), speed(60.0, 1_000.0)]), Some(0), "a full tie keeps the earlier option");
    }

    #[test]
    fn empty_and_invalid_inputs_choose_nothing_or_are_skipped() {
        assert_eq!(balanced_choice(&[]), None);
        let invalid = [speed(f64::NAN, 1_000.0), speed(0.0, 1_000.0), speed(50.0, -1.0), speed(f64::INFINITY, 500.0)];
        assert_eq!(balanced_choice(&invalid), None);
        let mixed = [speed(f64::NAN, 9_000.0), speed(50.0, 500.0), speed(80.0, 0.0), speed(49.0, 900.0)];
        assert_eq!(balanced_choice(&mixed), Some(3), "invalid options never become the base; the valid pair is judged alone");
    }
}
