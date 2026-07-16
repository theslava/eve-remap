use crate::calculator::{sp_rate_per_second, sp_for_level};
use crate::data::models::*;

/// Seconds in one day.
const SECS_PER_DAY: f64 = 86_400.0;

/// Default remap cooldown in days (paid account).
const REMAP_COOLDOWN_DAYS: f64 = 365.0;

/// Total attribute points available during a neural interface remap.
const REMAP_POINTS: u32 = 25;

/// Minimum value any single attribute may take after allocation.
const MIN_ATTR_VAL: u32 = 1;

/// Maximum value any single attribute may reach after allocation.
const MAX_ATTR_VAL: u32 = 25;

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------


/// Internal per-skill tracking during simulation.
#[derive(Debug, Clone)]
struct SkillSimEntry {
    skill_id: u32,
    name: String,
    current_level: u8,
    target_level: u8,
    remaining_sp: f64,
    record: SkillRecord,
}

/// Full simulation snapshot across all active skills in the queue.
#[derive(Debug, Clone)]
pub struct SimulationState {
    pub entries: Vec<SkillSimEntry>,
    pub elapsed_seconds: f64,
}

/// Outcome from simulating one epoch under a fixed allocation.
#[derive(Debug, Clone)]
struct EpochResult {
    completed: Vec<(u32, String)>,  // (skill_id, name) that finished this epoch
    state_after: SimulationState,
    seconds_used: f64,
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
                continue; // already max level — nothing to train
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
            entries,
            elapsed_seconds: 0.0,
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

/// Enumerate every valid attribute distribution summing to `total_points`.
///
/// Each of the five attributes must lie in `[min_attr..=max_attr]`.
/// Uses recursive backtracking with pruning so that partial sums never exceed
/// what is feasible for the remaining slots.
pub fn generate_allocations(
    total_points: u32,
    min_attr: u32,
    max_attr: u32,
) -> Vec<BaseAttributes> {
    let mut results = Vec::new();
    // Pre-allocate a rough upper bound: C(total_points + attrs - 1, attrs - 1)
    // For total=25,attrs=5 this is ~10K — generous but safe.
    results.reserve(16_000);
    let mut current = [min_attr; 5];
    backtrack(0, total_points, min_attr, max_attr, &mut current, &mut results);
    results
}

fn backtrack(
    idx: u32,
    points_left: u32,
    min_attr: u32,
    max_attr: u32,
    current: &mut [u32; 5],
    results: &mut Vec<BaseAttributes>,
) {
    if idx == 5 {
        if points_left == 0 {
            results.push(BaseAttributes {
                intelligence: current[0] as f64,
                charisma: current[1] as f64,
                perception: current[2] as f64,
                memory: current[3] as f64,
                willpower: current[4] as f64,
            });
        }
        return;
    }

    let slots_after = (4 - idx) as u32;
    let min_needed_for_rest = slots_after * min_attr;
    // We must leave at least `min_needed_for_rest` for the remaining attributes.
    let upper = (points_left.saturating_sub(min_needed_for_rest)).min(max_attr);
    // Also cannot go below min_attr itself.
    let lower = min_attr.max(points_left.saturating_sub(slots_after * max_attr));

    if lower > upper {
        return; // infeasible — prune this branch
    }

    for val in lower..=upper {
        current[idx as usize] = val;
        backtrack(idx + 1, points_left - val, min_attr, max_attr, current, results);
    }
}

// ---------------------------------------------------------------------------
// Epoch simulator
// ---------------------------------------------------------------------------

/// Advance all active skills under `effective_attrs` for up to `duration_secs`.
///
/// Skills complete one-by-one in chronological order. The simulation stops when
/// either every skill has finished or the time budget is exhausted.
pub fn simulate_epoch(
    mut state: SimulationState,
    effective_attrs: &EffectiveAttributes,
    duration_secs: f64,
) -> EpochResult {
    let start_elapsed = state.elapsed_seconds;
    let mut completed = Vec::new();
    let mut time_budget = duration_secs;

    while !state.entries.is_empty() && time_budget > 0.0 {
        // Find which entry finishes first (smallest remaining_sp / rate).
        let mut earliest_idx = None;
        let mut earliest_dt = f64::INFINITY;

        for (i, entry) in state.entries.iter().enumerate() {
            if entry.remaining_sp <= 0.0_f64 {
                continue;
            }
            let rate = sp_rate_per_second(&entry.record, effective_attrs);
            if rate <= 0.0 {
                continue;
            }
            let dt = entry.remaining_sp / rate;
            if dt < earliest_dt {
                earliest_dt = dt;
                earliest_idx = Some(i);
            }
        }

        match earliest_idx {
            None => break, // stuck — no active skill can make progress
            Some(idx) => {
                if earliest_dt >= time_budget {
                    // Advance all entries by the remaining budget and stop.
                    for entry in &mut state.entries {
                        if entry.remaining_sp > 0.0 {
                            let rate = sp_rate_per_second(&entry.record, effective_attrs);
                            entry.remaining_sp -= rate * time_budget;
                        }
                    }
                    state.elapsed_seconds += time_budget;
                    time_budget = 0.0;
                    break;
                }

                // Advance every entry to the completion point of the fastest one.
                for entry in &mut state.entries {
                    if entry.remaining_sp > 0.0 {
                        let rate = sp_rate_per_second(&entry.record, effective_attrs);
                        entry.remaining_sp -= rate * earliest_dt;
                    }
                }
                state.elapsed_seconds += earliest_dt;
                time_budget -= earliest_dt;

                // Pop the finished entry.
                let finished = state.entries.remove(idx);
                completed.push((finished.skill_id, finished.name));
            }
        }
    }

    let seconds = state.elapsed_seconds - start_elapsed;
    EpochResult {
        completed,
        state_after: state,
        seconds_used: seconds,
    }
}

// ---------------------------------------------------------------------------
// Greedy optimizer
// ---------------------------------------------------------------------------

/// Project how long it takes to finish all remaining skills under a **single**
/// fixed allocation (no further remaps). Returns total wall-clock seconds from
/// `state.elapsed_seconds`.
fn project_completion_time(
    state: &SimulationState,
    base_attrs: BaseAttributes,
    active_implant_ids: &[u32],
    implants: &[ImplantRecord],
) -> f64 {
    let effective = EffectiveAttributes::from_base_and_implants(
        &base_attrs,
        active_implant_ids,
        implants,
    );
    let result = simulate_epoch(state.clone(), &effective, f64::INFINITY);
    result.state_after.elapsed_seconds
}

/// Choose the best allocation for the *remaining* simulation by projecting each
/// candidate through all future epochs greedily and picking the one that yields
/// the earliest projected finish.
///
/// For the final epoch (when `next_remap_available` is None or unreachable), this
/// simply picks the single allocation finishing fastest.
///
/// When there are still future remaps available, we do a two-level greedy look-ahead:
/// 1. Simulate the current epoch with candidate A until the next remap boundary.
/// 2. From the post-epoch state, pick the best allocation for remaining skills
///    under the assumption of no further remaps (single-allocation projection).
/// The total projected time across both phases determines which candidate wins.
fn choose_best_allocation(
    state: &SimulationState,
    allocations: &[BaseAttributes],
    active_implant_ids: &[u32],
    implants: &[ImplantRecord],
    current_epoch_duration_secs: f64,
) -> BaseAttributes {
    if allocations.is_empty() {
        return BaseAttributes {
            intelligence: MIN_ATTR_VAL as f64,
            charisma: MIN_ATTR_VAL as f64,
            perception: MIN_ATTR_VAL as f64,
            memory: MIN_ATTR_VAL as f64,
            willpower: MIN_ATTR_VAL as f64,
        };
    }

    let mut best_alloc = allocations[0];
    let mut best_projected_finish = project_completion_time(state, best_alloc, active_implant_ids, implants);

    for alloc in &allocations[1..] {
        let finish = project_completion_time(state, *alloc, active_implant_ids, implants);
        if finish < best_projected_finish {
            best_projected_finish = finish;
            best_alloc = *alloc;
        }
    }

    // If we have a bounded epoch duration (i.e., there's a next remap), do a more
    // refined two-phase look-ahead only for the top candidates. This catches cases
    // where an allocation is suboptimal for the full run but excellent for this
    // specific epoch window because it accelerates skills that would otherwise block
    // future epochs.
    if current_epoch_duration_secs.is_finite() && current_epoch_duration_secs > 0.0 {
        // Rank all allocations by how much progress they make within this epoch.
        let mut scored: Vec<(f64, BaseAttributes)> = allocations
            .iter()
            .map(|a| {
                let effective = EffectiveAttributes::from_base_and_implants(
                    a,
                    active_implant_ids,
                    implants,
                );
                let result = simulate_epoch(state.clone(), &effective, current_epoch_duration_secs);
                // Score: negative of SP completed (more negative = better)
                let sp_completed: f64 = state.entries.iter().map(|e| e.remaining_sp).sum();
                let sp_remaining: f64 = result.state_after.entries.iter().map(|e| e.remaining_sp).sum();
                let progress = sp_completed - sp_remaining;
                (-progress, *a)
            })
            .collect();

        scored.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());

        // Take top-5 and do full two-phase projection for each.
        let top_n = scored.len().min(5);
        for i in 0..top_n {
            let alloc = scored[i].1;
            let effective_now = EffectiveAttributes::from_base_and_implants(
                &alloc,
                active_implant_ids,
                implants,
            );
            let epoch_result = simulate_epoch(
                state.clone(),
                &effective_now,
                current_epoch_duration_secs,
            );

            if epoch_result.state_after.entries.is_empty() {
                // All done — this is unbeatable.
                return alloc;
            }

            // Project remaining under best single allocation (no further remaps).
            let remaining_finish = project_completion_time(
                &epoch_result.state_after,
                alloc,
                active_implant_ids,
                implants,
            );

            if remaining_finish < best_projected_finish {
                best_projected_finish = remaining_finish;
                best_alloc = alloc;
            }
        }
    }

    best_alloc
}

/// Run the greedy multi-epoch remap optimizer.
///
/// The algorithm proceeds as follows:
///
/// 1. **Epoch 0** always uses the character's *current* base attributes. It runs
///    until either all skills complete or the next scheduled remap becomes available.
///
/// 2. At each subsequent remap boundary, every valid attribute allocation is
///    evaluated. The one minimizing projected wall-clock finish time for all
///    remaining skills wins. A new epoch begins with that allocation.
///
/// 3. After the last usable remap, a final epoch simulates to completion using the
///    best single allocation found for whatever remains.
pub fn optimize(
    char_state: &CharacterState,
    skills_db: &[SkillRecord],
    implants: &[ImplantRecord],
) -> OptimizationResult {
    let mut sim_state = char_state.build_simulation_state(skills_db);

    // Nothing to optimize — queue is empty or all skills are already at max level.
    if sim_state.entries.is_empty() {
        return OptimizationResult {
            epochs: Vec::new(),
            total_days: 0.0,
            total_wall_clock_seconds: 0.0,
        };
    }

    let initial_effective = char_state.effective_attributes(implants);
    let allocations = generate_allocations(REMAP_POINTS, MIN_ATTR_VAL, MAX_ATTR_VAL);

    let mut result_epochs = Vec::new();
    let mut next_remap_at_secs: f64 = REMAP_COOLDOWN_DAYS * SECS_PER_DAY;

    // -----------------------------------------------------------------------
    // Epoch 0: fixed to current character attributes (no remap wasted now).
    // -----------------------------------------------------------------------
    {
        let epoch_duration = (next_remap_at_secs - sim_state.elapsed_seconds).max(0.0);
        let epoch_result = simulate_epoch(sim_state.clone(), &initial_effective, epoch_duration);

        result_epochs.push(EpochPlan {
            start_offset_days: sim_state.elapsed_seconds / SECS_PER_DAY,
            attributes: char_state.base_attributes,
            effective_attributes: initial_effective,
            completed_skills: epoch_result.completed,
            projected_finish_days: epoch_result.state_after.elapsed_seconds / SECS_PER_DAY,
        });
        sim_state = epoch_result.state_after;
    }

    // -----------------------------------------------------------------------
    // Subsequent epochs: greedy allocation selection at each remap boundary.
    // -----------------------------------------------------------------------
    while !sim_state.entries.is_empty() {
        let time_until_next_remap = (next_remap_at_secs - sim_state.elapsed_seconds).max(0.0);

        if time_until_next_remap <= 0.0 || time_until_next_remap == f64::INFINITY {
            // No more meaningful remaps — pick best single allocation for the rest.
            let best_alloc = choose_best_allocation(
                &sim_state,
                &allocations,
                &char_state.active_implant_ids,
                implants,
                f64::INFINITY,
            );
            let final_effective = EffectiveAttributes::from_base_and_implants(
                &best_alloc,
                &char_state.active_implant_ids,
                implants,
            );
            let epoch_result = simulate_epoch(sim_state.clone(), &final_effective, f64::INFINITY);

            result_epochs.push(EpochPlan {
                start_offset_days: sim_state.elapsed_seconds / SECS_PER_DAY,
                attributes: best_alloc,
                effective_attributes: final_effective,
                completed_skills: epoch_result.completed,
                projected_finish_days: epoch_result.state_after.elapsed_seconds / SECS_PER_DAY,
            });
            sim_state = epoch_result.state_after;
            break;
        }

        // Choose best allocation considering this epoch's duration window.
        let chosen = choose_best_allocation(
            &sim_state,
            &allocations,
            &char_state.active_implant_ids,
            implants,
            time_until_next_remap,
        );
        let chosen_effective = EffectiveAttributes::from_base_and_implants(
            &chosen,
            &char_state.active_implant_ids,
            implants,
        );

        let epoch_result = simulate_epoch(sim_state.clone(), &chosen_effective, time_until_next_remap);

        result_epochs.push(EpochPlan {
            start_offset_days: sim_state.elapsed_seconds / SECS_PER_DAY,
            attributes: chosen,
            effective_attributes: chosen_effective,
            completed_skills: epoch_result.completed,
            projected_finish_days: epoch_result.state_after.elapsed_seconds / SECS_PER_DAY,
        });
        sim_state = epoch_result.state_after;

        next_remap_at_secs += REMAP_COOLDOWN_DAYS * SECS_PER_DAY;
    }

    let total_wall_clock = sim_state.elapsed_seconds;
    OptimizationResult {
        epochs: result_epochs,
        total_days: total_wall_clock / SECS_PER_DAY,
        total_wall_clock_seconds: total_wall_clock,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- helpers ----------------------------------------------------------

    fn make_skill(primary: Attribute, secondary: Attribute, stc: f64) -> SkillRecord {
        SkillRecord {
            id: 1001,
            name: "TestSkill".to_string(),
            primary_attribute: primary,
            secondary_attribute: secondary,
            skill_time_constant: stc,
        }
    }

    fn attrs(int: f64, cha: f64, per: f64, mem: f64, wil: f64) -> EffectiveAttributes {
        EffectiveAttributes {
            intelligence: int,
            charisma: cha,
            perception: per,
            memory: mem,
            willpower: wil,
        }
    }

    fn base_attrs(int: f64, cha: f64, per: f64, mem: f64, wil: f64) -> BaseAttributes {
        BaseAttributes {
            intelligence: int,
            charisma: cha,
            perception: per,
            memory: mem,
            willpower: wil,
        }
    }

    /// Clamp arbitrary attribute values into a valid distribution summing to REMAP_POINTS.
    fn clamp_to_valid(int: f64, cha: f64, per: f64, mem: f64, wil: f64) -> BaseAttributes {
        let sum = int + cha + per + mem + wil;
        if sum <= 5.0 {
            return base_attrs(1.0, 1.0, 1.0, 1.0, 1.0);
        }
        let excess = sum - 5.0;
        let scale = 20.0 / excess.clamp(0.001, f64::MAX);
        base_attrs(
            (int - 1.0).clamp(0.0, 24.0) * scale + 1.0,
            (cha - 1.0).clamp(0.0, 24.0) * scale + 1.0,
            (per - 1.0).clamp(0.0, 24.0) * scale + 1.0,
            (mem - 1.0).clamp(0.0, 24.0) * scale + 1.0,
            (wil - 1.0).clamp(0.0, 24.0) * scale + 1.0,
        )
    }

    // -- allocation generator tests ---------------------------------------

    #[test]
    fn test_allocation_count_minimal() {
        // total=5, min=1, max=25 → only one way: [1,1,1,1,1]
        let allocs = generate_allocations(5, 1, 25);
        assert_eq!(allocs.len(), 1);
        assert!((allocs[0].total() - 5.0).abs() < 1e-9);
    }

    #[test]
    fn test_allocation_count_small() {
        // total=6, min=1, max=25 → C(5,4)=5 ways
        // [2,1,1,1,1], [1,2,1,1,1], [1,1,2,1,1], [1,1,1,2,1], [1,1,1,1,2]
        let allocs = generate_allocations(6, 1, 25);
        assert_eq!(allocs.len(), 5);
    }

    #[test]
    fn test_allocation_count_standard_remap() {
        // total=25, min=1, max=25 → C(24,4) = 10_626
        // (max_attr doesn't constrain because no single attr can exceed 21 when others are at 1)
        let allocs = generate_allocations(25, 1, 25);
        assert_eq!(allocs.len(), 10_626);

        // Verify every allocation is valid.
        for a in &allocs {
            assert!((a.total() - 25.0).abs() < 1e-9);
            assert!(a.intelligence >= 1.0 && a.intelligence <= 25.0);
            assert!(a.charisma >= 1.0 && a.charisma <= 25.0);
            assert!(a.perception >= 1.0 && a.perception <= 25.0);
            assert!(a.memory >= 1.0 && a.memory <= 25.0);
            assert!(a.willpower >= 1.0 && a.willpower <= 25.0);
        }
    }

    #[test]
    fn test_allocation_with_tight_max() {
        // total=10, min=1, max=3 → constrained by upper bound
        // All solutions to x1+..+x5=10 with 1<=xi<=3:
        // Possible patterns (sorted): [3,3,2,1,1], [3,2,2,2,1], [2,2,2,2,2]
        // Permutations of [3,3,2,1,1]: 5!/(2!*1!*2!) = 30
        // Permutations of [3,2,2,2,1]: 5!/(1!*3!*1!) = 20
        // Permutations of [2,2,2,2,2]: 1
        // Total = 51
        let allocs = generate_allocations(10, 1, 3);
        assert_eq!(allocs.len(), 51);

        for a in &allocs {
            assert!(a.intelligence >= 1.0 && a.intelligence <= 3.0);
            assert!(a.charisma >= 1.0 && a.charisma <= 3.0);
            assert!(a.perception >= 1.0 && a.perception <= 3.0);
            assert!(a.memory >= 1.0 && a.memory <= 3.0);
            assert!(a.willpower >= 1.0 && a.willpower <= 3.0);
        }
    }

    #[test]
    fn test_allocation_includes_expected_values() {
        let allocs = generate_allocations(6, 1, 25);
        // Should contain the allocation where all points are on intelligence.
        let has_all_int = allocs.iter().any(|a| {
            (a.intelligence - 2.0).abs() < 1e-9
                && (a.charisma - 1.0).abs() < 1e-9
                && (a.perception - 1.0).abs() < 1e-9
                && (a.memory - 1.0).abs() < 1e-9
                && (a.willpower - 1.0).abs() < 1e-9
        });
        assert!(has_all_int, "Missing [2,1,1,1,1] allocation");
    }

    // -- simulation tests -------------------------------------------------

    #[test]
    fn test_simulate_epoch_single_skill_completes() {
        let skill = make_skill(Attribute::Intelligence, Attribute::Memory, 4.0);
        // Level 1→2: SP needed = LEVEL_MULTIPLIERS[0] * STC = 1.0 * 4.0 = 4.0 SP
        let state = SimulationState {
            entries: vec![SkillSimEntry {
                skill_id: 1001,
                name: "TestSkill".to_string(),
                current_level: 1,
                target_level: 2,
                remaining_sp: 4.0,
                record: skill.clone(),
            }],
            elapsed_seconds: 0.0,
        };
        // Rate = (primary + secondary/2) / 60 = (10 + 6/2)/60 = 13/60 ≈ 0.2167 SP/s
        // Time to finish = 4.0 / (13/60) = 240/13 ≈ 18.46 seconds
        let result = simulate_epoch(state, &attrs(10.0, 1.0, 1.0, 6.0, 1.0), f64::INFINITY);

        assert_eq!(result.completed.len(), 1);
        assert_eq!(result.completed[0].0, 1001);
        assert!((result.seconds_used - 18.46).abs() < 0.5);
        assert!(result.state_after.entries.is_empty());
    }

    #[test]
    fn test_simulate_epoch_time_budget_exhausted() {
        let skill = make_skill(Attribute::Intelligence, Attribute::Memory, 4.0);
        // Level 1→2 needs 4.0 SP at rate (13/60) SP/s → ~18.46s total
        let state = SimulationState {
            entries: vec![SkillSimEntry {
                skill_id: 1001,
                name: "TestSkill".to_string(),
                current_level: 1,
                target_level: 2,
                remaining_sp: 4.0,
                record: skill.clone(),
            }],
            elapsed_seconds: 0.0,
        };
        // Only give 5 seconds — not enough to finish.
        let result = simulate_epoch(state, &attrs(10.0, 1.0, 1.0, 6.0, 1.0), 5.0);

        assert!(result.completed.is_empty());
        assert!((result.seconds_used - 5.0).abs() < 1e-9);
        // Remaining SP ≈ 4.0 - (13/60)*5.0 = 4.0 - 1.0833 ≈ 2.917
        assert!((result.state_after.entries[0].remaining_sp - 2.917).abs() < 0.1);
    }

    #[test]
    fn test_simulate_epoch_multiple_skills_ordering() {
        // Skill A: INT primary, MEM secondary, STC=2 → faster with high INT attrs
        let skill_a = make_skill(Attribute::Intelligence, Attribute::Memory, 2.0);
        // Skill B: CHA primary, PER secondary, STC=2 → slower with low CHA/PER attrs
        let mut skill_b_record = make_skill(Attribute::Charisma, Attribute::Perception, 2.0);
        skill_b_record.id = 1002;
        skill_b_record.name = "SkillB".to_string();

        // Both at level 1→2 needing 2.0 SP each (multiplier[0]=1.0 * STC=2.0)
        let state = SimulationState {
            entries: vec![
                SkillSimEntry {
                    skill_id: 1001,
                    name: "SkillA".to_string(),
                    current_level: 1,
                    target_level: 2,
                    remaining_sp: 2.0,
                    record: skill_a.clone(),
                },
                SkillSimEntry {
                    skill_id: 1002,
                    name: "SkillB".to_string(),
                    current_level: 1,
                    target_level: 2,
                    remaining_sp: 2.0,
                    record: skill_b_record.clone(),
                },
            ],
            elapsed_seconds: 0.0,
        };
        // INT=20, MEM=5, CHA=1, PER=1, WIL=1 → sum=28 (not a valid remap but fine for testing)
        // Rate A = (20 + 5/2)/60 = 22.5/60 = 0.375 SP/s → time_A = 2.0/0.375 ≈ 5.33s
        // Rate B = (1 + 1/2)/60 = 1.5/60 = 0.025 SP/s → time_B = 2.0/0.025 = 80s
        let result = simulate_epoch(
            state,
            &attrs(20.0, 1.0, 1.0, 5.0, 1.0),
            f64::INFINITY,
        );

        // Skill A should finish first.
        assert_eq!(result.completed.len(), 2);
        assert_eq!(result.completed[0].0, 1001); // Skill A finishes first
        assert_eq!(result.completed[1].0, 1002); // Skill B finishes second
    }

    #[test]
    fn test_simulate_epoch_empty_state() {
        let state = SimulationState {
            entries: vec![],
            elapsed_seconds: 0.0,
        };
        let result = simulate_epoch(state, &attrs(5.0, 5.0, 5.0, 5.0, 5.0), 1000.0);
        assert!(result.completed.is_empty());
        assert!((result.seconds_used - 0.0).abs() < 1e-9);
    }

    #[test]
    fn test_simulate_epoch_preserves_elapsed_time() {
        // Start with elapsed time already advanced and verify it accumulates.
        let skill = make_skill(Attribute::Intelligence, Attribute::Memory, 4.0);
        let state = SimulationState {
            entries: vec![SkillSimEntry {
                skill_id: 1001,
                name: "TestSkill".to_string(),
                current_level: 1,
                target_level: 2,
                remaining_sp: 4.0,
                record: skill.clone(),
            }],
            elapsed_seconds: 1000.0, // pretend we've already been running for a while
        };
        let result = simulate_epoch(state, &attrs(10.0, 1.0, 1.0, 6.0, 1.0), f64::INFINITY);

        assert_eq!(result.completed.len(), 1);
        // Elapsed should be original + simulation time (~18.46s).
        assert!((result.state_after.elapsed_seconds - (1000.0 + 18.46)).abs() < 0.5);
    }

    // -- optimizer integration tests --------------------------------------

    #[test]
    fn test_optimize_empty_queue() {
        let char_state = CharacterState {
            base_attributes: base_attrs(5.0, 5.0, 5.0, 5.0, 5.0),
            active_implant_ids: vec![],
            queued_skills: vec![],
            effective_attributes: attrs(5.0, 5.0, 5.0, 5.0, 5.0),
        };
        let skills_db: Vec<SkillRecord> = vec![];
        let implants: Vec<ImplantRecord> = vec![];

        let result = optimize(&char_state, &skills_db, &implants);
        assert!(result.epochs.is_empty());
        assert_eq!(result.total_days, 0.0);
    }

    #[test]
    fn test_optimize_single_skill_basic() {
        let skill = make_skill(Attribute::Intelligence, Attribute::Memory, 4.0);
        let skills_db = vec![skill];
        let implants: Vec<ImplantRecord> = vec![];

        let char_state = CharacterState {
            base_attributes: base_attrs(10.0, 3.0, 3.0, 7.0, 2.0), // sum=25
            active_implant_ids: vec![],
            queued_skills: vec![QueuedSkill {
                id: 1001,
                level: 1,
                sp: 4_000_000,          // total SP for this transition
                duration: 600,           // 10 minutes total (arbitrary)
                remaining_sec: 600,      // not started yet
                is_active: true,
            }],
            effective_attributes: attrs(10.0, 3.0, 3.0, 7.0, 2.0),
        };

        let result = optimize(&char_state, &skills_db, &implants);
        assert!(!result.epochs.is_empty());
        assert!(result.total_days > 0.0);
        assert!(result.total_wall_clock_seconds > 0.0);

        // Epoch 0 should have completed the skill or made progress toward it.
        let mut skills_completed = 0;
        for epoch in &result.epochs {
            skills_completed += epoch.completed_skills.len();
        }
        assert_eq!(skills_completed, 1);
    }

    #[test]
    fn test_optimize_multi_epoch_progression() {
        // Create two skills with different primary attributes so remapping matters.
        let skill_int = make_skill(Attribute::Intelligence, Attribute::Memory, 100.0);
        let mut skill_mem_record = make_skill(Attribute::Memory, Attribute::Willpower, 100.0);
        skill_mem_record.id = 1002;
        skill_mem_record.name = "MemSkill".to_string();
        let skills_db = vec![skill_int, skill_mem_record];
        let implants: Vec<ImplantRecord> = vec![];

        // Character has high INT but low MEM — will train INT skill fast, MEM slow.
        let char_state = CharacterState {
            base_attributes: clamp_to_valid(20.0, 1.0, 1.0, 3.0, 0.0),
            active_implant_ids: vec![],
            queued_skills: vec![
                QueuedSkill {
                    id: 1001,
                    level: 1,
                    sp: 4_000_000,
                    duration: 86400 * 7,  // ~1 week (arbitrary)
                    remaining_sec: 86400 * 7,
                    is_active: true,
                },
                QueuedSkill {
                    id: 1002,
                    level: 1,
                    sp: 4_000_000,
                    duration: 86400 * 30, // ~month (arbitrary)
                    remaining_sec: 86400 * 30,
                    is_active: false,
                },
            ],
            effective_attributes: attrs(20.0, 1.0, 1.0, 3.0, 1.0),
        };

        let result = optimize(&char_state, &skills_db, &implants);
        assert!(result.epochs.len() >= 1);

        // Total skills completed should be the queue length.
        let mut total_completed = 0;
        for epoch in &result.epochs {
            total_completed += epoch.completed_skills.len();
        }
        assert_eq!(total_completed, 2);
    }

    #[test]
    fn test_optimize_with_implants() {
        // Create an implant that gives +3 to Intelligence.
        let mut bonuses = std::collections::HashMap::new();
        bonuses.insert(Attribute::Intelligence, 3);
        let implant = ImplantRecord {
            type_id: 9000,
            name: "Test Implant".to_string(),
            bonuses,
        };
        let implants = vec![implant];

        let skill = make_skill(Attribute::Intelligence, Attribute::Memory, 4.0);
        let skills_db = vec![skill];
        let char_state = CharacterState {
            base_attributes: base_attrs(10.0, 3.0, 3.0, 7.0, 2.0),
            active_implant_ids: vec![9000], // activate the INT+3 implant
            queued_skills: vec![QueuedSkill {
                id: 1001,
                level: 1,
                sp: 4_000_000,
                duration: 600,
                remaining_sec: 600,
                is_active: true,
            }],
            effective_attributes: attrs(13.0, 3.0, 3.0, 7.0, 2.0), // INT boosted by +3 implant
        };

        let result = optimize(&char_state, &skills_db, &implants);
        assert!(!result.epochs.is_empty());

        // Epoch 0 should reflect effective INT of 13 (base 10 + implant 3).
        let epoch0 = &result.epochs[0];
        assert!((epoch0.effective_attributes.intelligence - 13.0).abs() < 0.1);
    }
}
