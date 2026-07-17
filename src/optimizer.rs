use std::time::Instant;
use crate::calculator::{sp_rate_per_second, sp_for_level};
use crate::data::models::*;

/// Seconds in one day.
const SECS_PER_DAY: f64 = 86_400.0;

/// Default remap cooldown in days (paid account).
const REMAP_COOLDOWN_DAYS: f64 = 365.0;

/// Base attribute value before any remapping. Hard floor — cannot go lower.
const BASE_ATTR_VAL: u32 = 17;

/// Free points to distribute above base during each neural interface remap.
const REMAP_FREE_POINTS: u32 = 14;

/// Maximum additional points on any single attribute from one remap.
const MAX_ADD_PER_ATTR: u32 = 10;

/// Total sum of all five attributes after a valid allocation (= 17*5 + 14 = 99).
const ATTR_SUM: u32 = BASE_ATTR_VAL * 5 + REMAP_FREE_POINTS; // 99

/// Minimum value for any attribute after allocation.
const MIN_ATTR_AFTER_REMAP: u32 = BASE_ATTR_VAL; // 17

/// Maximum value for any attribute after allocation.
const MAX_ATTR_AFTER_REMAP: u32 = BASE_ATTR_VAL + MAX_ADD_PER_ATTR; // 27

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// Single skill being trained, tracking remaining SP toward its target level.
#[derive(Debug, Clone)]
struct SkillSimEntry {
    skill_id: u32,
    name: String,
    current_level: u8,
    target_level: u8,
    /// Remaining SP to earn for this skill→level transition.
    remaining_sp: f64,
    record: SkillRecord,
}

/// Snapshot of the sequential training queue during simulation.
///
/// Skills train **one at a time** in order — `entries[0]` is the currently active
/// skill, and it carries partial progress across epoch boundaries.
#[derive(Debug, Clone)]
pub struct SimulationState {
    pub entries: Vec<SkillSimEntry>,
    /// Index of the currently active skill (the one earning SP right now).
    pub active_index: usize,
    /// Total wall-clock seconds elapsed so far.
    pub elapsed_seconds: f64,
}

impl SimulationState {
    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn len(&self) -> usize {
        self.entries.len()
    }
}

// ---------------------------------------------------------------------------
// CharacterState helpers
// ---------------------------------------------------------------------------

impl CharacterState {
    /// Build an initial `SimulationState` from the character's queue and SDE db.
    fn build_simulation_state(&self, skills_db: &[SkillRecord]) -> SimulationState {
        let mut entries = Vec::with_capacity(self.queued_skills.len());

        for qs in &self.queued_skills {
            if qs.level >= 5 {
                continue; // already max level
            }

            let Some(record) = skills_db.iter().find(|r| r.id == qs.id) else {
                continue; // unknown skill — skip
            };

            let target_level = qs.level + 1;
            let total_sp = sp_for_level(record, qs.level, target_level);

            // How much SP is already earned toward this transition?
            let progress = qs.progress_fraction();
            let earned_sp = progress * total_sp;
            let remaining_sp = (total_sp - earned_sp).max(0.0);

            entries.push(SkillSimEntry {
                skill_id: qs.id,
                name: record.name.clone(),
                current_level: qs.level,
                target_level,
                remaining_sp,
                record: record.clone(),
            });
        }

        SimulationState {
            active_index: 0,
            elapsed_seconds: 0.0,
            entries,
        }
    }

    /// Derive effective attributes from base values plus active implants.
    fn effective_attributes(&self, implants: &[ImplantRecord]) -> EffectiveAttributes {
        EffectiveAttributes::from_base_and_implants(
            &self.base_attributes,
            &self.active_implant_ids,
            implants,
        )
    }
}

// ---------------------------------------------------------------------------
// Allocation generator
// ---------------------------------------------------------------------------

/// Enumerate every valid attribute distribution for a remap.
///
/// Each attribute must be in [17..=27], sum of added points (= attr-17) across all five must equal 14.
pub fn generate_allocations() -> Vec<BaseAttributes> {
    let mut results = Vec::new();
    // Five nested loops are fine — each iterates at most 11 values (17..=27).
    // Total iterations: 11^5 = 161,051 — fast enough with pruning.
    for int in BASE_ATTR_VAL..=MAX_ATTR_AFTER_REMAP {
        for cha in BASE_ATTR_VAL..=MAX_ATTR_AFTER_REMAP {
            for per in BASE_ATTR_VAL..=MAX_ATTR_AFTER_REMAP {
                for mem in BASE_ATTR_VAL..=MAX_ATTR_AFTER_REMAP {
                    let partial_sum = int + cha + per + mem;
                    if partial_sum > ATTR_SUM {
                        break; // further mem values only increase the sum
                    }
                    let wil = ATTR_SUM - partial_sum;
                    if wil >= MIN_ATTR_AFTER_REMAP && wil <= MAX_ATTR_AFTER_REMAP {
                        results.push(BaseAttributes {
                            intelligence: int as f64,
                            charisma: cha as f64,
                            perception: per as f64,
                            memory: mem as f64,
                            willpower: wil as f64,
                        });
                    }
                }
            }
        }
    }
    results
}

// ---------------------------------------------------------------------------
// Sequential simulation engine
// ---------------------------------------------------------------------------

/// Train a single skill under `effective_attrs` and return seconds consumed.
fn train_one_skill(entry: &SkillSimEntry, effective_attrs: &EffectiveAttributes) -> f64 {
    let rate = sp_rate_per_second(&entry.record, effective_attrs);
    if rate <= 0.0 {
        return f64::INFINITY; // stuck
    }
    entry.remaining_sp / rate
}

/// Project total wall-clock seconds to finish all remaining skills under one fixed allocation.
/// Skills train sequentially — sum of individual training times.
fn project_total_time(
    state: &SimulationState,
    effective_attrs: &EffectiveAttributes,
) -> f64 {
    let mut total_secs = 0.0;
    for i in state.active_index..state.entries.len() {
        let secs = train_one_skill(&state.entries[i], effective_attrs);
        if secs.is_infinite() {
            return f64::INFINITY;
        }
        total_secs += secs;
    }
    total_secs
}

/// Simulate sequential training under `effective_attrs` for up to `duration_secs`.
///
/// Processes skills one-by-one from the active index. Each skill either completes
/// (advancing to the next) or partially trains and pauses at the epoch boundary.
/// Returns which skills completed and the updated simulation state.
pub fn simulate_epoch(
    mut state: SimulationState,
    effective_attrs: &EffectiveAttributes,
    duration_secs: f64,
) -> EpochResult {
    let start_elapsed = state.elapsed_seconds;
    let mut completed = Vec::new();
    let mut time_remaining = duration_secs;
    // Track total SP earned per (role × attribute) for this epoch.
    let mut sp_summary = AttributeSpSummary::default();
    while !state.is_empty() && time_remaining > 0.0 {
        // Ensure active_index is valid
        if state.active_index >= state.len() {
            break;
        }

        let entry = &state.entries[state.active_index];
        let secs_needed = train_one_skill(entry, effective_attrs);

        if secs_needed <= time_remaining {
            // Skill completes within this epoch's window.
            state.elapsed_seconds += secs_needed;
            time_remaining -= secs_needed;

            let finished = state.entries.remove(state.active_index);
            let sp_earned = finished.remaining_sp;
            completed.push((finished.skill_id, finished.name.clone(), secs_needed));

            // Accumulate SP into primary/secondary buckets.
            let pri_key = finished.record.primary_attribute.to_string();
            *sp_summary.primary.entry(pri_key).or_insert(0.0) += sp_earned;
            let sec_key = finished.record.secondary_attribute.to_string();
            *sp_summary.secondary.entry(sec_key).or_insert(0.0) += sp_earned;

            // Don't advance active_index — remove shifts later entries down.
        } else {
            // Skill doesn't finish in this window — advance its progress.
            let remaining_sp_before = state.entries[state.active_index].remaining_sp;
            let rate = sp_rate_per_second(&state.entries[state.active_index].record, effective_attrs);
            let earned = rate * time_remaining;
            state.entries[state.active_index].remaining_sp = (remaining_sp_before - earned).max(0.0);
            state.elapsed_seconds += time_remaining;
            break;
        }
    }

    let seconds_used = state.elapsed_seconds - start_elapsed;
    EpochResult {
        completed,
        state_after: state,
        seconds_used,
        sp_summary,
    }
}

/// Outcome from simulating one epoch under a fixed allocation.
struct EpochResult {
    /// Skills that fully completed during this epoch (in order of completion).
    completed: Vec<(u32, String, f64)>, // (skill_id, name, seconds_to_train)
    state_after: SimulationState,
    seconds_used: f64,
    /// Total SP per (role × attribute) pair for completed skills.
    sp_summary: AttributeSpSummary,
}

// ---------------------------------------------------------------------------
// Greedy optimizer
// ---------------------------------------------------------------------------

/// Choose the best allocation by projecting total finish time for all remaining skills
/// under each candidate and picking the minimum.
fn choose_best_allocation(
    state: &SimulationState,
    allocations: &[BaseAttributes],
    implants: &[ImplantRecord],
) -> Option<BaseAttributes> {
    if allocations.is_empty() || state.is_empty() {
        return None;
    }

    let mut best_alloc = allocations[0];
    let best_effective = EffectiveAttributes::from_base_and_implants(
        &best_alloc,
        &[], // implants already baked into attrs via effective_attributes call below
        implants,
    );
    let mut best_total_secs = project_total_time(state, &best_effective);

    for alloc in &allocations[1..] {
        let effective = EffectiveAttributes::from_base_and_implants(alloc, &[], implants);
        let total_secs = project_total_time(state, &effective);
        if total_secs < best_total_secs {
            best_total_secs = total_secs;
            best_alloc = *alloc;
        }
    }

    Some(best_alloc)
}

/// Simulate spending N bonus remaps back-to-back from the current state.
/// Each bonus pick selects the best allocation for remaining skills and trains
/// until completion — no cooldown waits between them.
/// Returns a vector of epochs produced by the bonus runs.
fn simulate_bonus_remaps(
    mut sim_state: SimulationState,
    allocations: &[BaseAttributes],
    implants: &[ImplantRecord],
    implant_bonus: &BaseAttributes,
    active_implant_ids: &[u32],
    num_bonuses: u32,
) -> Vec<EpochPlan> {
    let mut epochs = Vec::new();
    let mut bonuses_left = num_bonuses;

    while !sim_state.is_empty() && bonuses_left > 0 {
        let Some(chosen) = choose_best_allocation(&sim_state, allocations, implants) else {
            break;
        };
        let chosen_with_implants = chosen.add(implant_bonus);
        let chosen_effective = EffectiveAttributes::from_base_and_implants(
            &chosen_with_implants,
            active_implant_ids,
            implants,
        );

        // Train to completion with this allocation (no time limit).
        let epoch_result = simulate_epoch(sim_state.clone(), &chosen_effective, f64::INFINITY);

        epochs.push(EpochPlan {
            start_offset_secs: sim_state.elapsed_seconds,
            attributes: chosen,
            effective_attributes: chosen_effective,
            completed_skills: epoch_result.completed,
            projected_finish_secs: epoch_result.state_after.elapsed_seconds,
            bonus_remaps_used: 1,
            sp_summary: epoch_result.sp_summary,
        });

        sim_state = epoch_result.state_after;
        bonuses_left -= 1;
    }

    // If skills remain after all bonuses are spent, finish under last alloc.
    if !sim_state.is_empty() && !epochs.is_empty() {
        // Use the same allocation as the last bonus epoch for remaining skills.
        let last_alloc = epochs.last().unwrap().attributes;
        let last_eff = epochs.last().unwrap().effective_attributes.clone();
        let epoch_result = simulate_epoch(sim_state.clone(), &last_eff, f64::INFINITY);
        epochs.push(EpochPlan {
            start_offset_secs: sim_state.elapsed_seconds,
            attributes: last_alloc,
            effective_attributes: last_eff,
            completed_skills: epoch_result.completed,
            projected_finish_secs: epoch_result.state_after.elapsed_seconds,
            bonus_remaps_used: 0,
            sp_summary: epoch_result.sp_summary,
        });
    }

    epochs
}

/// Try to decide whether spending bonus remaps now beats waiting for timed boundaries.
/// Compares total finish time of "spend all N bonuses greedily" vs "wait for next timed remap".
/// Returns Some(epochs) if bonus strategy is faster, None otherwise.
fn try_bonus_remap_strategy(
    mut sim_state: SimulationState,
    allocations: &[BaseAttributes],
    implants: &[ImplantRecord],
    implant_bonus: &BaseAttributes,
    active_implant_ids: &[u32],
    num_bonuses: u32,
) -> Option<Vec<EpochPlan>> {
    if num_bonuses == 0 || sim_state.is_empty() {
        return None;
    }

    // Strategy A: spend all bonuses greedily right now
    let bonus_epochs = simulate_bonus_remaps(
        sim_state.clone(),
        allocations,
        implants,
        implant_bonus,
        active_implant_ids,
        num_bonuses,
    );
    let bonus_finish_time = bonus_epochs.last().map_or(f64::INFINITY, |e| e.projected_finish_secs);

    // Strategy B: wait for the next timed remap boundary (one epoch only).
    // We just need a rough comparison — one timed epoch vs immediate bonus.
    let timed_finish_time = {
        let Some(chosen) = choose_best_allocation(&sim_state, allocations, implants) else {
            return None;
        };
        let chosen_with_implants = chosen.add(implant_bonus);
        let chosen_effective = EffectiveAttributes::from_base_and_implants(
            &chosen_with_implants,
            active_implant_ids,
            implants,
        );
        project_total_time(&sim_state, &chosen_effective) + sim_state.elapsed_seconds
    };

    if bonus_finish_time < timed_finish_time {
        Some(bonus_epochs)
    } else {
        None
    }
}
/// Run the greedy multi-epoch remap optimizer with sequential training.
pub fn optimize(
    char_state: &CharacterState,
    skills_db: &[SkillRecord],
    implants: &[ImplantRecord],
) -> OptimizationResult {
    let _timer = Instant::now();
    eprintln!("[+] Starting optimization...");
    
    let mut sim_state = char_state.build_simulation_state(skills_db);

    // Nothing to optimize — queue is empty or all skills are at max level.
    if sim_state.is_empty() {
        return OptimizationResult {
            epochs: Vec::new(),
            total_wall_clock_seconds: 0.0,
            baseline_wall_clock_seconds: 0.0,
        };
    }

    let initial_effective = char_state.effective_attributes(implants);

    // Baseline: total wall-clock seconds with current attrs, no remaps at all.
    let baseline_secs = project_total_time(&sim_state, &initial_effective);
    let allocations = generate_allocations();

    eprintln!(
        "[+] Queue: {} skills to train across {} levels",
        sim_state.len(),
        count_level_transitions(&sim_state.entries)
    );
    eprintln!(
        "[+] Allocation space: {} valid distributions",
        allocations.len()
    );

    let mut result_epochs = Vec::new();
    let mut next_remap_at_secs: f64 = REMAP_COOLDOWN_DAYS * SECS_PER_DAY;

    // -----------------------------------------------------------------------
    // Epoch 0: fixed to current character attributes. No reason to waste a
    // remap immediately when the queue is fresh and we haven't optimized yet.
    // -----------------------------------------------------------------------
    {
        let epoch_duration = if sim_state.elapsed_seconds < next_remap_at_secs {
            next_remap_at_secs - sim_state.elapsed_seconds
        } else {
            f64::INFINITY // already past cooldown — treat as free remap window
        };
        let epoch_result = simulate_epoch(sim_state.clone(), &initial_effective, epoch_duration);
        result_epochs.push(EpochPlan {
            start_offset_secs: sim_state.elapsed_seconds,
            attributes: char_state.base_attributes,
            effective_attributes: initial_effective,
            completed_skills: epoch_result.completed.clone(),
            projected_finish_secs: epoch_result.state_after.elapsed_seconds,
            bonus_remaps_used: 0,
            sp_summary: epoch_result.sp_summary,
        });
        eprintln!(
            "[+] Epoch 0 (current attrs): {} skills done, {:.1}s wall",
            epoch_result.completed.len(),
            _timer.elapsed().as_secs_f64()
        );
        sim_state = epoch_result.state_after;
    }

    // -----------------------------------------------------------------------
    // Bonus remap check: try spending available bonuses now vs waiting.
    // -----------------------------------------------------------------------
    let bonus_remaps_available = char_state.bonus_remaps.unwrap_or(0);
    if !sim_state.is_empty() && bonus_remaps_available > 0 {
        if let Some(bonus_epochs) = try_bonus_remap_strategy(
            sim_state.clone(),
            &allocations,
            implants,
            &char_state.implant_bonus,
            &char_state.active_implant_ids,
            bonus_remaps_available,
        ) {
            eprintln!(
                "[+] Bonus remap strategy chosen over timed wait ({:.1}s wall)",
                _timer.elapsed().as_secs_f64()
            );
            result_epochs.extend(bonus_epochs);
            // Update sim_state from the last bonus epoch's finish point.
            if let Some(last_epoch) = result_epochs.last() {
                sim_state.elapsed_seconds = last_epoch.projected_finish_secs;
                sim_state.entries.clear();
            }
        } else {
            eprintln!("[+] Timed remap strategy preferred (no bonus advantage)");
        }
    }

    // -----------------------------------------------------------------------
    // Subsequent epochs: one per timed remap boundary. Each picks the best
    // allocation for remaining skills and runs until the next 365-day mark
    // or queue completion.
    // -----------------------------------------------------------------------
    while !sim_state.is_empty() {
        next_remap_at_secs += REMAP_COOLDOWN_DAYS * SECS_PER_DAY;
        let Some(chosen) = choose_best_allocation(&sim_state, &allocations, implants) else {
            break;
        };
        // Add implant bonus back on top of the new remap allocation.
        let chosen_with_implants = chosen.add(&char_state.implant_bonus);
        let chosen_effective = EffectiveAttributes::from_base_and_implants(
            &chosen_with_implants,
            &char_state.active_implant_ids,
            implants,
        );

        // Run this epoch until the next cooldown boundary (or to completion).
        let epoch_duration = if sim_state.elapsed_seconds < next_remap_at_secs {
            next_remap_at_secs - sim_state.elapsed_seconds
        } else {
            f64::INFINITY
        };

        let epoch_result = simulate_epoch(sim_state.clone(), &chosen_effective, epoch_duration);

        result_epochs.push(EpochPlan {
            start_offset_secs: sim_state.elapsed_seconds,
            attributes: chosen,
            effective_attributes: chosen_effective,
            completed_skills: epoch_result.completed.clone(),
            projected_finish_secs: epoch_result.state_after.elapsed_seconds,
            bonus_remaps_used: 0,
            sp_summary: epoch_result.sp_summary,
        });

        eprintln!(
            "[+] Epoch {} (attrs {:?}): {} skills done ({:.1}s wall)",
            result_epochs.len() - 1,
            format_attrs(&chosen),
            epoch_result.completed.len(),
            _timer.elapsed().as_secs_f64()
        );

        sim_state = epoch_result.state_after;
    }

        let total_wall_clock = sim_state.elapsed_seconds;
    eprintln!(
        "[+] Optimization complete: {} epochs in {:.2}s",
        result_epochs.len(),
        _timer.elapsed().as_secs_f64()
    );

    OptimizationResult {
        epochs: result_epochs,
        total_wall_clock_seconds: total_wall_clock,
        baseline_wall_clock_seconds: baseline_secs,
    }
}

/// Count the number of distinct level transitions in a queue.
fn count_level_transitions(entries: &[SkillSimEntry]) -> usize {
    entries.iter().filter(|e| e.remaining_sp > 0.0).count()
}

/// Format attribute allocation as a compact string for logging.
fn format_attrs(attrs: &BaseAttributes) -> String {
    format!(
        "I{:.0}/C{:.0}/P{:.0}/M{:.0}/W{:.0}",
        attrs.intelligence,
        attrs.charisma,
        attrs.perception,
        attrs.memory,
        attrs.willpower,
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_skill(primary: Attribute, secondary: Attribute, stc: f64) -> SkillRecord {
        SkillRecord {
            id: 1001, name: "TestSkill".to_string(),
            primary_attribute: primary, secondary_attribute: secondary, skill_time_constant: stc,
        }
    }

    fn base_attrs(int: f64, cha: f64, per: f64, mem: f64, wil: f64) -> BaseAttributes {
        BaseAttributes { intelligence: int, charisma: cha, perception: per, memory: mem, willpower: wil }
    }

    fn char_state(attrs: BaseAttributes, skills: Vec<QueuedSkill>, bonus_remaps: Option<u32>) -> CharacterState {
        CharacterState {
            base_attributes: attrs, queued_skills: skills,
            active_implant_ids: Vec::new(),
            implant_bonus: BaseAttributes { intelligence: 0., charisma: 0., perception: 0., memory: 0., willpower: 0. },
            effective_attributes: EffectiveAttributes::from(attrs),
            bonus_remaps,
        }
    }

    /// Create a queue entry with realistic SP/duration so progress_fraction works.
    fn qe(id: u32, level: u8, total_sp: f64) -> QueuedSkill {
        let dur = (total_sp / 0.5).ceil() as u64;
        QueuedSkill { id, level, sp: 0, duration: dur.max(1), remaining_sec: dur.max(1), is_active: true }
    }

    // -- allocation generator tests ---------------------------------------

    #[test]
    fn test_allocation_count_standard_remap() {
        let allocs = generate_allocations();
        assert_eq!(allocs.len(), 2_885);
        for a in &allocs {
            let sum = (a.intelligence + a.charisma + a.perception + a.memory + a.willpower).round() as u32;
            assert_eq!(sum, ATTR_SUM);
            assert!((a.intelligence as u32) >= MIN_ATTR_AFTER_REMAP && (a.intelligence as u32) <= MAX_ATTR_AFTER_REMAP);
            assert!((a.charisma as u32) >= MIN_ATTR_AFTER_REMAP && (a.charisma as u32) <= MAX_ATTR_AFTER_REMAP);
            assert!((a.perception as u32) >= MIN_ATTR_AFTER_REMAP && (a.perception as u32) <= MAX_ATTR_AFTER_REMAP);
            assert!((a.memory as u32) >= MIN_ATTR_AFTER_REMAP && (a.memory as u32) <= MAX_ATTR_AFTER_REMAP);
            assert!((a.willpower as u32) >= MIN_ATTR_AFTER_REMAP && (a.willpower as u32) <= MAX_ATTR_AFTER_REMAP);
        }
    }

    #[test]
    fn test_allocation_no_single_attr_dump() {
        let allocs = generate_allocations();
        for a in &allocs {
            let boosted = [a.intelligence, a.charisma, a.perception, a.memory, a.willpower]
                .iter().filter(|&&v| v > BASE_ATTR_VAL as f64).count();
            assert!(boosted >= 2, "must boost at least 2 attributes");
        }
    }

    // -- sequential simulation tests --------------------------------------

    #[test]
    fn test_simulate_epoch_empty_state() {
        let state = SimulationState { entries: vec![], active_index: 0, elapsed_seconds: 0.0 };
        let eff = EffectiveAttributes::from(base_attrs(17.,17.,17.,17.,17.));
        let result = simulate_epoch(state, &eff, SECS_PER_DAY * 365.0);
        assert!(result.completed.is_empty());
        assert_eq!(result.seconds_used, 0.0);
    }

    #[test]
    fn test_simulate_epoch_single_skill_completes() {
        let skill = make_skill(Attribute::Intelligence, Attribute::Memory, 1.0);
        let total_sp = sp_for_level(&skill, 1, 2);
        let entry = SkillSimEntry {
            skill_id: skill.id, name: skill.name.clone(), current_level: 1, target_level: 2,
            remaining_sp: total_sp, record: skill,
        };
        let state = SimulationState { entries: vec![entry], active_index: 0, elapsed_seconds: 0.0 };
        let eff = EffectiveAttributes::from(base_attrs(27., 17., 17., 17., 17.));
        let result = simulate_epoch(state, &eff, f64::INFINITY);
        assert_eq!(result.completed.len(), 1);
        assert!(result.state_after.is_empty());
    }

    #[test]
    fn test_simulate_epoch_sequential_order() {
        let skill_a = make_skill(Attribute::Intelligence, Attribute::Memory, 1.0);
        let skill_b = SkillRecord { id: 2002, name: "SkillB".to_string(), primary_attribute: Attribute::Charisma, secondary_attribute: Attribute::Willpower, skill_time_constant: 1.0 };
        let sp_a = sp_for_level(&skill_a, 1, 2);
        let sp_b = sp_for_level(&skill_b, 1, 2);
        let state = SimulationState {
            entries: vec![
                SkillSimEntry { skill_id: skill_a.id, name: skill_a.name.clone(), current_level: 1, target_level: 2, remaining_sp: sp_a, record: skill_a },
                SkillSimEntry { skill_id: skill_b.id, name: skill_b.name.clone(), current_level: 1, target_level: 2, remaining_sp: sp_b, record: skill_b },
            ], active_index: 0, elapsed_seconds: 0.0,
        };
        let eff = EffectiveAttributes::from(base_attrs(17., 17., 17., 17., 17.));
        let result = simulate_epoch(state, &eff, f64::INFINITY);
        assert_eq!(result.completed.len(), 2);
        assert_eq!(result.completed[0].0, 1001); // A before B (queue order)
        assert_eq!(result.completed[1].0, 2002);
    }

    #[test]
    fn test_simulate_epoch_time_budget_exhausted() {
        let skill = make_skill(Attribute::Intelligence, Attribute::Memory, 5.0);
        let total_sp = sp_for_level(&skill, 1, 2);
        let state = SimulationState { entries: vec![SkillSimEntry { skill_id: skill.id, name: skill.name.clone(), current_level: 1, target_level: 2, remaining_sp: total_sp, record: skill }], active_index: 0, elapsed_seconds: 0.0 };
        let eff = EffectiveAttributes::from(base_attrs(17., 17., 17., 17., 17.));
        let result = simulate_epoch(state, &eff, 60.0); // 1 minute — not enough
        assert!(result.completed.is_empty());
        assert!(result.state_after.entries[0].remaining_sp < total_sp);
    }

    #[test]
    fn test_project_total_time_sequential() {
        let skill_a = make_skill(Attribute::Intelligence, Attribute::Memory, 1.0);
        let skill_b = SkillRecord { id: 2002, name: "SkillB".to_string(), primary_attribute: Attribute::Charisma, secondary_attribute: Attribute::Willpower, skill_time_constant: 1.0 };
        let sp_a = sp_for_level(&skill_a, 1, 2);
        let sp_b = sp_for_level(&skill_b, 1, 2);
        let state = SimulationState { entries: vec![
            SkillSimEntry { skill_id: skill_a.id, name: skill_a.name.clone(), current_level: 1, target_level: 2, remaining_sp: sp_a, record: skill_a },
            SkillSimEntry { skill_id: skill_b.id, name: skill_b.name.clone(), current_level: 1, target_level: 2, remaining_sp: sp_b, record: skill_b },
        ], active_index: 0, elapsed_seconds: 0.0 };
        let eff = EffectiveAttributes::from(base_attrs(17., 17., 17., 17., 17.));
        let rate_a = sp_rate_per_second(&state.entries[0].record, &eff);
        let rate_b = sp_rate_per_second(&state.entries[1].record, &eff);
        let expected = (sp_a / rate_a) + (sp_b / rate_b);
        let actual = project_total_time(&state, &eff);
        assert!((actual - expected).abs() < 1e-6);
    }

    #[test]
    fn test_choose_best_prefers_primary_attr() {
        let skill = make_skill(Attribute::Intelligence, Attribute::Memory, 1.0);
        let total_sp = sp_for_level(&skill, 1, 2);
        let state = SimulationState { entries: vec![SkillSimEntry { skill_id: skill.id, name: skill.name.clone(), current_level: 1, target_level: 2, remaining_sp: total_sp, record: skill }], active_index: 0, elapsed_seconds: 0.0 };
        let allocations = generate_allocations();
        let best = choose_best_allocation(&state, &allocations, &[]).unwrap();
        assert!(best.intelligence >= 25.0, "INT should be near max for INT-primary skill");
    }

    #[test]
    fn test_optimize_empty_queue() {
        let char_st = char_state(base_attrs(17., 17., 17., 17., 17.), Vec::new(), None);
        let result = optimize(&char_st, &[], &[]);
        assert_eq!(result.total_wall_clock_seconds, 0.0);
        assert!(result.epochs.is_empty());
    }

    #[test]
    fn test_optimize_single_skill_basic() {
        let skill = make_skill(Attribute::Intelligence, Attribute::Memory, 1.0);
        let sp_needed = sp_for_level(&skill, 1, 2);
        let char_st = char_state(base_attrs(17., 17., 17., 17., 17.), vec![qe(skill.id, 1, sp_needed)], Some(1));
        let result = optimize(&char_st, &[skill], &[]);
        assert!(!result.epochs.is_empty());
        assert!(result.total_wall_clock_seconds > 0.0);
    }

    #[test]
    fn test_simulate_epoch_preserves_elapsed_time() {
        let skill = make_skill(Attribute::Willpower, Attribute::Perception, 3.0);
        let total_sp = sp_for_level(&skill, 1, 2);
        let state = SimulationState { entries: vec![SkillSimEntry { skill_id: skill.id, name: skill.name.clone(), current_level: 1, target_level: 2, remaining_sp: total_sp, record: skill }], active_index: 0, elapsed_seconds: 1000.0 };
        let eff = EffectiveAttributes::from(base_attrs(17., 17., 17., 17., 17.));
        let half_time = 5000.0;
        let result = simulate_epoch(state.clone(), &eff, half_time);
        assert!((result.state_after.elapsed_seconds - (state.elapsed_seconds + half_time)).abs() < 1e-6);
    }

    #[test]
    fn test_optimize_multi_epoch_progression() {
        let mut skills_db = Vec::new();
        let mut queued_skills = Vec::new();
        for i in 0..30u32 {
            let primary = if i % 2 == 0 { Attribute::Intelligence } else { Attribute::Memory };
            let secondary = if i % 2 == 0 { Attribute::Memory } else { Attribute::Intelligence };
            let skill = SkillRecord { id: 3000 + i, name: format!("Skill{}", i), primary_attribute: primary, secondary_attribute: secondary, skill_time_constant: 2.0 };
            skills_db.push(skill.clone());
            let sp_needed = sp_for_level(&skill, 1, 2);
            let dur = (sp_needed / 0.5).ceil() as u64;
            queued_skills.push(QueuedSkill { id: skill.id, level: 1, sp: 0, duration: dur.max(1), remaining_sec: dur.max(1), is_active: i == 0 });
        }
        let char_st = char_state(base_attrs(17., 17., 17., 17., 17.), queued_skills, Some(2));
        let result = optimize(&char_st, &skills_db, &[]);
        assert!(!result.epochs.is_empty());
        assert!(result.total_wall_clock_seconds > 0.0);
    }

    #[test]
    fn test_optimize_with_implants() {
        let skill = make_skill(Attribute::Intelligence, Attribute::Memory, 1.0);
        let mut bonuses = std::collections::HashMap::new();
        bonuses.insert(Attribute::Intelligence, 4);
        let implants = vec![ImplantRecord { type_id: 5001, name: "Talisman Delta".to_string(), bonuses }];
        let sp_needed = sp_for_level(&skill, 1, 2);
        let char_st = char_state(base_attrs(17., 17., 17., 17., 17.), vec![qe(skill.id, 1, sp_needed)], Some(1));
        let result = optimize(&char_st, &[skill], &implants);
        assert!(result.total_wall_clock_seconds > 0.0);
    }

    #[test]
    fn test_allocation_distribution_by_boosted_count() {
        let allocs = generate_allocations();
        use std::collections::HashMap;
        let mut dist = HashMap::new();
        for a in &allocs {
            let boosted = [a.intelligence, a.charisma, a.perception, a.memory, a.willpower]
                .iter().filter(|&&v| v > BASE_ATTR_VAL as f64).count();
            *dist.entry(boosted).or_insert(0usize) += 1;
        }
        assert!(!dist.contains_key(&1));
        // Verify distribution sums to total
        assert_eq!(dist.values().sum::<usize>(), allocs.len());
    }

    #[test]
    fn test_sequential_first_skill_matters_most() {
        // First skill has large SP cost — its rate dominates total time.
        let skill_int = SkillRecord { id: 5001, name: "BigINTSkill".to_string(), primary_attribute: Attribute::Intelligence, secondary_attribute: Attribute::Memory, skill_time_constant: 10.0 };
        let skill_wil = SkillRecord { id: 5002, name: "TinyWILSkill".to_string(), primary_attribute: Attribute::Willpower, secondary_attribute: Attribute::Perception, skill_time_constant: 0.5 };
        let sp_int = sp_for_level(&skill_int, 1, 2); // huge
        let sp_wil = sp_for_level(&skill_wil, 1, 2); // small
        let state = SimulationState { entries: vec![
            SkillSimEntry { skill_id: skill_int.id, name: skill_int.name.clone(), current_level: 1, target_level: 2, remaining_sp: sp_int, record: skill_int },
            SkillSimEntry { skill_id: skill_wil.id, name: skill_wil.name.clone(), current_level: 1, target_level: 2, remaining_sp: sp_wil, record: skill_wil },
        ], active_index: 0, elapsed_seconds: 0.0 };
        // INT-heavy alloc should beat WIL-heavy when first skill is INT-primary and large.
        let eff_int = EffectiveAttributes::from(base_attrs(27., 21., 17., 17., 17.));
        let eff_wil = EffectiveAttributes::from(base_attrs(17., 17., 17., 17., 27.));
        let time_with_int = project_total_time(&state, &eff_int);
        let time_with_wil = project_total_time(&state, &eff_wil);
        assert!(time_with_int < time_with_wil, "INT-first heavy queue benefits from INT-heavy allocation");
    }
}
