use std::time::Instant;
use crate::calculator::{sp_rate_per_second, sp_for_level};
use crate::data::models::*;
/// Seconds in one day (used by tests).
#[allow(dead_code)]
const SECS_PER_DAY: f64 = 86_400.0;

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

/// Single skill being trained, tracking remaining SP toward its level transition.
#[derive(Debug, Clone)]
struct SkillSimEntry {
    skill_id: u32,
    name: String,
    #[allow(dead_code)]
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
    #[allow(dead_code)]
    pub active_index: usize,
    #[allow(dead_code)]
    pub elapsed_seconds: f64,
}

impl SimulationState {
    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[allow(dead_code)]
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

    /// Derive effective attributes from base values plus implant bonuses.
    fn effective_attributes(&self, implants: &[ImplantRecord]) -> EffectiveAttributes {
        // Start with base + direct implant bonus (--implant-bonuses in offline mode).
        let base_with_bonuses = self.base_attributes.add(&self.implant_bonus);
        EffectiveAttributes::from_base_and_implants(
            &base_with_bonuses,
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
#[allow(dead_code)]
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
#[allow(dead_code)]
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
#[allow(dead_code)]
struct EpochResult {
    /// Skills that fully completed during this epoch (in order of completion).
    completed: Vec<(u32, String, f64)>, // (skill_id, name, seconds_to_train)
    state_after: SimulationState,
    #[allow(dead_code)]
    seconds_used: f64,
    /// Total SP per (role × attribute) pair for completed skills.
    sp_summary: AttributeSpSummary,
}

// ---------------------------------------------------------------------------
// Greedy optimizer
// ---------------------------------------------------------------------------

/// Choose the best allocation by projecting total finish time for all remaining skills
/// under each candidate and picking the minimum.
#[allow(dead_code)]
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

/// Reorder skill training queue for attribute locality while respecting prerequisites.
///
/// Uses topological sort over prerequisite edges; when multiple skills are ready,
/// picks the one whose (primary, secondary) pair matches the most remaining skills —
/// clustering same-attribute work together so remaps cover more ground per switch.
/// Skills matching the strongest current attributes are front-loaded first.
fn reorder_queue(
    entries: Vec<SkillSimEntry>,
    skills_db: &[SkillRecord],
    initial_effective: &EffectiveAttributes,
) -> Vec<SkillSimEntry> {
    // Build lookup from skill_id → (primary, secondary) for all SDE skills.
    let attr_map: std::collections::HashMap<u32, (Attribute, Attribute)> = skills_db.iter()
        .map(|r| (r.id, (r.primary_attribute, r.secondary_attribute)))
        .collect();

    // Collect set of skill IDs present in our queue (for quick lookup).
    let queued_ids: std::collections::HashSet<u32> = entries.iter().map(|e| e.skill_id).collect();

    // Build adjacency: for each entry index, how many unsatisfied prereqs?
    let n = entries.len();
    let mut in_degree: Vec<usize> = vec![0; n];
    let mut reverse_deps: Vec<Vec<usize>> = vec![Vec::new(); n];

    // Add implicit sequential edges within same skill: L→(N+1) must come after L→N.
    // Group entries by skill_id, sort by target_level, chain them.
    {
        let mut by_skill: std::collections::HashMap<u32, Vec<usize>> =
            std::collections::HashMap::new();
        for i in 0..n {
            by_skill.entry(entries[i].skill_id).or_default().push(i);
        }
        for indices in by_skill.values_mut() {
            indices.sort_by_key(|&i| entries[i].target_level);
            for w in indices.windows(2) {
                reverse_deps[w[0]].push(w[1]);
                in_degree[w[1]] += 1;
            }
        }
    }

    // Add explicit prerequisite edges from SDE data.
    for i in 0..n {
        let entry = &entries[i];
        for &(req_id, req_level) in &entry.record.prerequisites {
            if !queued_ids.contains(&req_id) {
                continue;
            }
            // Find the queued entry that trains this prerequisite to at least req_level.
            // Among candidates, pick the one with highest target_level <= req_level
            // (the last level before or at what we need), or any >= req_level.
            let dominated_by = (0..n).filter(|&j| j != i && entries[j].skill_id == req_id)
                .max_by_key(|&j| {
                    if entries[j].target_level >= req_level {
                        u32::MAX // fully satisfies — prefer it
                    } else {
                        entries[j].target_level as u32 // partial but still needed first
                    }
                });

            if let Some(j) = dominated_by {
                reverse_deps[j].push(i);
                in_degree[i] += 1;
            }
        }
    }
    // Kahn's algorithm with attribute-aware tie-breaking.
    let mut ready: Vec<usize> = (0..n).filter(|&i| in_degree[i] == 0).collect();
    let mut ordered = Vec::with_capacity(n);

    while !ready.is_empty() {
        // Score each ready entry on two axes:
        //   1) How well its (primary, secondary) pair matches the strongest current
        //      attributes — high-rate skills go first so they benefit from the
        //      current allocation before we remap away.
        //   2) Clustering bonus for matching the last scheduled skill's attributes,
        //      keeping same-attribute work contiguous.
        let chosen_pos = ready.iter().copied().enumerate().max_by_key(|&(_pos, idx)| {
            let (p, s) = attr_map.get(&entries[idx].skill_id)
                .copied()
                .unwrap_or((Attribute::Intelligence, Attribute::Memory));

            // Axis 1: effective SP rate under current attrs (higher = faster training now).
            let eff_primary = initial_effective.get(p);
            let eff_secondary = initial_effective.get(s);
            let rate_score = ((eff_primary + eff_secondary / 2.0) * 1_000_000.0) as u32;

            // Axis 2: clustering bonus — count other unscheduled entries sharing p or s.
            let total_unscheduled = n - ordered.len();
            let mut cluster_score: u32 = 0;
            for k in &ready {
                if *k == idx { continue; }
                let (jp, js) = attr_map.get(&entries[*k].skill_id).copied()
                    .unwrap_or((Attribute::Intelligence, Attribute::Memory));
                if jp == p || js == s {
                    cluster_score += 1;
                }
            }
            // Strong continuity bonus if last scheduled skill shares attributes.
            if let Some(last_idx) = ordered.last().map(|i| *i as usize) {
                let (lp, ls) = attr_map.get(&entries[last_idx].skill_id).copied()
                    .unwrap_or((Attribute::Intelligence, Attribute::Memory));
                if lp == p || ls == s {
                    cluster_score += total_unscheduled as u32;
                }
            }

            // Combine: rate is the dominant key so high-rate skills lead;
            // cluster breaks ties among similar-rate skills.
            rate_score * (n + 1) as u32 + cluster_score
        }).map(|(pos, _)| pos).unwrap_or(0);
        let chosen = ready[chosen_pos];
        ready.remove(chosen_pos);
        ordered.push(chosen);

        // Release dependents of the chosen entry.
        for &dep in &reverse_deps[chosen] {
            in_degree[dep] -= 1;
            if in_degree[dep] == 0 {
                ready.push(dep);
            }
        }
    }

    // If not all entries were scheduled (cycle), append remaining in original order.
    if ordered.len() < n {
        for i in 0..n {
            if !ordered.contains(&i) {
                ordered.push(i);
            }
        }
    }

    ordered.into_iter().map(|idx| entries[idx].clone()).collect()
}

/// Run the greedy multi-epoch remap optimizer with sequential training.
pub fn optimize(
    char_state: &CharacterState,
    skills_db: &[SkillRecord],
    implants: &[ImplantRecord],
) -> OptimizationResult {
    let _timer = Instant::now();
    eprintln!("[+] Starting optimization...");

    let sim_state = char_state.build_simulation_state(skills_db);

    // Compute effective attributes early — needed by reorder_queue tie-breaking.
    let initial_effective = char_state.effective_attributes(implants);

    // Reorder queue for attribute locality while respecting prerequisites.
    let entries = if sim_state.entries.len() > 1 {
        reorder_queue(sim_state.entries.clone(), skills_db, &initial_effective)
    } else {
        sim_state.entries.clone()
    };

    // Nothing to optimize — queue is empty or all skills are at max level.
    if entries.is_empty() {
        return OptimizationResult {
            epochs: Vec::new(),
            total_wall_clock_seconds: 0.0,
            baseline_wall_clock_seconds: 0.0,
        };
    }

    let n = entries.len();

    // Baseline: train everything under current attrs, no remaps.
    let baseline_secs = {
        let mut t = 0.0;
        for entry in &entries {
            t += train_one_skill(entry, &initial_effective);
        }
        t
    };

    let allocations = generate_allocations();
    let alloc_count = allocations.len();

    eprintln!(
        "[+] Queue: {} skills to train across {} levels",
        n,
        count_level_transitions(&entries)
    );
    eprintln!(
        "[+] Allocation space: {} valid distributions",
        alloc_count
    );

    // ── Precompute per-skill training times ────────────────────────────────
    // time_cache[i * alloc_count + a] = seconds for entries[i] under allocation a.
    let mut time_cache = vec![0.0; n * alloc_count];

    // Effective attributes for each candidate allocation (with implants added).
    let effective_for_alloc: Vec<EffectiveAttributes> = allocations.iter().map(|alloc| {
        let with_implants = alloc.add(&char_state.implant_bonus);
        EffectiveAttributes::from_base_and_implants(
            &with_implants,
            &char_state.active_implant_ids,
            implants,
        )
    }).collect();

    for i in 0..n {
        for a in 0..alloc_count {
            time_cache[i * alloc_count + a] =
                train_one_skill(&entries[i], &effective_for_alloc[a]);
        }
    }

    // ── Precompute suffix sums per allocation ──────────────────────────────
    // suffix_sum[a][i] = sum of time_cache[k*alloc_count + a] for k in i..n
    let mut suffix_sum: Vec<Vec<f64>> = (0..alloc_count)
        .map(|_| vec![0.0; n + 1])
        .collect();

    for a in 0..alloc_count {
        for i in (0..n).rev() {
            suffix_sum[a][i] = suffix_sum[a][i + 1] + time_cache[i * alloc_count + a];
        }
    }

    // ── Main optimization loop ───────────────────────────────────────────────
    let mut result_epochs = Vec::new();
    let mut current_effective = initial_effective;
    let mut current_base = char_state.base_attributes;
    let mut bonus_left = char_state.bonus_remaps.unwrap_or(0);
    let mut remaining_start = 0usize;
    let mut wall_clock = 0.0f64;

    while remaining_start < n {
        // Compute "stay" times on the fly for current effective attrs.
        let mut stay_times: Vec<f64> = vec![0.0; n - remaining_start];
        let mut running_stay = 0.0;
        for (local_i, global_i) in (remaining_start..n).enumerate() {
            let t = train_one_skill(&entries[global_i], &current_effective);
            stay_times[local_i] = t;
            running_stay += t;
        }
        let stay_finish = wall_clock + running_stay;

        // No more bonus remaps — train to completion.
        if bonus_left == 0 {
            push_epoch_with_times(
                &mut result_epochs,
                remaining_start, n,
                wall_clock, stay_finish,
                &current_base, &current_effective,
                &entries, &stay_times,
            );
            eprintln!(
                "[+] Epoch {} (attrs {:?}): {} skills done ({:.1}s wall)",
                result_epochs.len(),
                format_attrs(&current_effective),
                n - remaining_start,
                _timer.elapsed().as_secs_f64()
            );
            break;
        }

        // ── Strategic cut-point search ──────────────────────────────────────
        // For each split point `cut` in (remaining_start+1 .. n):
        //   before = cumsum of stay_times up to cut
        //   after  = best suffix_sum[a][cut] across all allocations a
        // Pick whichever minimizes total finish time.
        let mut best_cut_info: Option<(usize, f64, usize)> = None; // (cut, total_finish, chosen_alloc_col)
        let mut cum_before = 0.0;

        for offset in 0..(n - remaining_start - 1) {
            cum_before += stay_times[offset];
            let cut = remaining_start + offset + 1;

            let mut best_after = suffix_sum[0][cut];
            let mut best_a = 0usize;
            for a in 1..alloc_count {
                if suffix_sum[a][cut] < best_after {
                    best_after = suffix_sum[a][cut];
                    best_a = a;
                }
            }

            let total_finish = wall_clock + cum_before + best_after;

            match &best_cut_info {
                None if total_finish < stay_finish - 1.0 => {
                    best_cut_info = Some((cut, total_finish, best_a));
                }
                Some((_, best_total, _)) if total_finish < *best_total - 1.0 => {
                    best_cut_info = Some((cut, total_finish, best_a));
                }
                _ => {}
            }
        }

        if let Some((cut, total_finish, chosen_a)) = best_cut_info {
            // Recompute exact before time for the winning cut.
            let epoch_end = wall_clock + cum_before_for(&stay_times, cut - remaining_start);

            push_epoch_with_times(
                &mut result_epochs,
                remaining_start, cut,
                wall_clock, epoch_end,
                &current_base, &current_effective,
                &entries, &stay_times,
            );
            result_epochs.last_mut().unwrap().bonus_remaps_used += 1;

            eprintln!(
                "[+] Epoch {} (attrs {:?}): {} skills done, then remap — saves {:.1}s ({:.1}d) ({:.1}s wall)",
                result_epochs.len(),
                format_attrs(&current_effective),
                cut - remaining_start,
                stay_finish - total_finish,
                (stay_finish - total_finish) / 86_400.0,
                _timer.elapsed().as_secs_f64()
            );

            bonus_left -= 1;
            current_base = allocations[chosen_a];
            current_effective = effective_for_alloc[chosen_a];
            wall_clock = epoch_end;
            remaining_start = cut;
        } else {
            // No beneficial switch found — train to completion under current attrs.
            push_epoch_with_times(
                &mut result_epochs,
                remaining_start, n,
                wall_clock, stay_finish,
                &current_base, &current_effective,
                &entries, &stay_times,
            );
            eprintln!(
                "[+] Epoch {} (attrs {:?}): {} skills done ({:.1}s wall)",
                result_epochs.len(),
                format_attrs(&current_effective),
                n - remaining_start,
                _timer.elapsed().as_secs_f64()
            );
            break;
        }
    }

    let total_wall_clock = result_epochs.last()
        .map(|e| e.projected_finish_secs)
        .unwrap_or(wall_clock);

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

/// Sum the first `count` elements of a slice.
fn cum_before_for(slice: &[f64], count: usize) -> f64 {
    let mut s = 0.0;
    for i in 0..count {
        s += slice[i];
    }
    s
}

/// Build an `EpochPlan` for entries[start..end) using precomputed training times.
fn push_epoch_with_times(
    epochs: &mut Vec<EpochPlan>,
    start: usize,
    end: usize,
    wall_clock: f64,
    projected_finish: f64,
    base: &BaseAttributes,
    effective: &EffectiveAttributes,
    entries: &[SkillSimEntry],
    train_times: &[f64],
) {
    let mut completed_skills = Vec::with_capacity(end - start);
    let mut sp_summary = AttributeSpSummary::default();

    for (local_i, global_i) in (start..end).enumerate() {
        let entry = &entries[global_i];
        let secs = train_times[local_i];
        completed_skills.push((entry.skill_id, entry.name.clone(), secs));

        let sp_earned = entry.remaining_sp;
        let pri_key = entry.record.primary_attribute.to_string();
        *sp_summary.primary.entry(pri_key).or_insert(0.0) += sp_earned;
        let sec_key = entry.record.secondary_attribute.to_string();
        *sp_summary.secondary.entry(sec_key).or_insert(0.0) += sp_earned;
    }

    epochs.push(EpochPlan {
        start_offset_secs: wall_clock,
        attributes: *base,
        effective_attributes: *effective,
        completed_skills,
        projected_finish_secs: projected_finish,
        bonus_remaps_used: 0,
        sp_summary,
    });
}

/// Count the number of distinct level transitions in a queue.
fn count_level_transitions(entries: &[SkillSimEntry]) -> usize {
    entries.iter().filter(|e| e.remaining_sp > 0.0).count()
}

/// Format effective attributes as a compact string for logging (implants included).
fn format_attrs(eff: &EffectiveAttributes) -> String {
    format!(
        "P{:.0}/M{:.0}/W{:.0}/I{:.0}/C{:.0}",
        eff.perception,
        eff.memory,
        eff.willpower,
        eff.intelligence,
        eff.charisma,
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_skill(primary: Attribute, secondary: Attribute, stc: f64) -> SkillRecord {
        SkillRecord { id: 1001, name: "TestSkill".to_string(), primary_attribute: primary, secondary_attribute: secondary, skill_time_constant: stc, prerequisites: vec![] }
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
        let skill_b = SkillRecord { id: 2002, name: "SkillB".to_string(), primary_attribute: Attribute::Charisma, secondary_attribute: Attribute::Willpower, skill_time_constant: 1.0, prerequisites: vec![] };
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
        let skill_b = SkillRecord { id: 2002, name: "SkillB".to_string(), primary_attribute: Attribute::Charisma, secondary_attribute: Attribute::Willpower, skill_time_constant: 1.0, prerequisites: vec![] };
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
            let skill = SkillRecord { id: 3000 + i, name: format!("Skill{}", i), primary_attribute: primary, secondary_attribute: secondary, skill_time_constant: 2.0, prerequisites: vec![] };
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
        let skill_int = SkillRecord { id: 5001, name: "BigINTSkill".to_string(), primary_attribute: Attribute::Intelligence, secondary_attribute: Attribute::Memory, skill_time_constant: 10.0, prerequisites: vec![] };
        let skill_wil = SkillRecord { id: 5002, name: "TinyWILSkill".to_string(), primary_attribute: Attribute::Willpower, secondary_attribute: Attribute::Perception, skill_time_constant: 0.5, prerequisites: vec![] };
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
