//! Bulk edit of monster areas. Pure: no egui, no files. A rule is *where* + *action*, written as
//! one line: `map:1-5 mob:12 => count +1`.
//!
//! * **Where** is the Find query (`search.rs`) over monster areas, with the words `map:` and `count:`. Every word must match.
//! * **Action**: `count +n`, `count -n`, `count = n`, `count * f` (rounded half up), `replace X with Y`, `add X *n`, `remove X`,
//!   `delete`, `grow d` / `shrink d` (each side), `slab h`. A count action touches the entries named by the `mob:` words, or every
//!   entry when the query has none.
//! * Counts stay in 1 to 8 unless the plan allows more. A count that would drop below 1 removes the entry; an area left with no
//!   entries stays.
//!
//! [`plan`] says what a rule would do to each map; [`MapPlan::ops`] turns it into the edit ops of one undo step.

use std::collections::HashSet;

use shaiya_svmap::{MobArea, MobEntry, SectionId};

use crate::edit::{self, Op, Rec};
use crate::search::{self, Env, MapIn, Query, Token};

/// The largest count an area has in the originals.
pub const MAX_COUNT: u32 = 8;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CountOp {
    Add(u32),
    Sub(u32),
    Set(u32),
    Mul(f32),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Count(CountOp),
    Replace { from: u32, to: u32 },
    Add { mob: u32, count: u32 },
    Remove(u32),
    Delete,
    /// Moves each side out by this much (negative: in). Written `grow d` or `shrink d`.
    Grow(f32),
    Slab(f32),
}

impl Action {
    pub fn text(&self) -> String {
        match self {
            Action::Count(CountOp::Add(n)) => format!("count +{n}"),
            Action::Count(CountOp::Sub(n)) => format!("count -{n}"),
            Action::Count(CountOp::Set(n)) => format!("count = {n}"),
            Action::Count(CountOp::Mul(f)) => format!("count * {f}"),
            Action::Replace { from, to } => format!("replace {from} with {to}"),
            Action::Add { mob, count } => format!("add {mob} *{count}"),
            Action::Remove(m) => format!("remove {m}"),
            Action::Delete => "delete".to_string(),
            Action::Grow(d) if *d < 0.0 => format!("shrink {}", -d),
            Action::Grow(d) => format!("grow {d}"),
            Action::Slab(h) => format!("slab {h}"),
        }
    }

    pub fn parse(text: &str) -> Result<Action, String> {
        let t = text.trim();
        // `get` and not `[..5]`: byte 5 of a typed text may sit inside a character
        let (verb, rest) = if t.get(..5).is_some_and(|p| p.eq_ignore_ascii_case("count")) && t.as_bytes().get(5).is_some_and(|b| !b.is_ascii_alphabetic()) {
            (&t[..5], t[5..].trim())
        } else {
            t.split_once(char::is_whitespace).map_or((t, ""), |(a, b)| (a, b.trim()))
        };
        let num = |s: &str, what: &str| -> Result<f32, String> { s.trim().parse::<f32>().ok().filter(|v| v.is_finite()).ok_or_else(|| format!("{what}: `{}` is not a number", s.trim())) };
        let int = |s: &str, what: &str| -> Result<u32, String> { s.trim().parse::<u32>().map_err(|_| format!("{what}: `{}` is not a whole number", s.trim())) };
        match verb.to_lowercase().as_str() {
            "count" => {
                let mut ch = rest.chars();
                let op = ch.next().ok_or("count needs +n, -n, = n or * f")?;
                let arg = ch.as_str();
                match op {
                    '+' | '-' => {
                        let n = int(arg, "count")?;
                        if n == 0 {
                            return Err("count: the step must be 1 or more".into());
                        }
                        Ok(Action::Count(if op == '+' { CountOp::Add(n) } else { CountOp::Sub(n) }))
                    }
                    '=' => {
                        let n = int(arg, "count")?;
                        if n == 0 {
                            return Err("count = 0: use remove <monster> to take a monster out".into());
                        }
                        Ok(Action::Count(CountOp::Set(n)))
                    }
                    '*' => {
                        let f = num(arg, "count")?;
                        if f <= 0.0 {
                            return Err("count *: the factor must be above 0".into());
                        }
                        Ok(Action::Count(CountOp::Mul(f)))
                    }
                    _ => Err("count needs +n, -n, = n or * f".into()),
                }
            }
            "replace" => {
                let words: Vec<&str> = rest.split_whitespace().collect();
                match words.as_slice() {
                    [a, w, b] if w.eq_ignore_ascii_case("with") => {
                        let (from, to) = (int(a, "replace")?, int(b, "replace")?);
                        if from == to {
                            return Err("replace: pick two different monsters".into());
                        }
                        Ok(Action::Replace { from, to })
                    }
                    _ => Err("replace needs: replace <monster> with <monster>".into()),
                }
            }
            "add" => {
                let (m, c) = rest.split_once('*').ok_or("add needs: add <monster> *<count>")?;
                let (mob, count) = (int(m, "add")?, int(c, "add")?);
                if count == 0 {
                    return Err("add: the count must be 1 or more".into());
                }
                Ok(Action::Add { mob, count })
            }
            "remove" => Ok(Action::Remove(int(rest, "remove")?)),
            "delete" if rest.is_empty() => Ok(Action::Delete),
            "delete" => Err("delete takes nothing after it".into()),
            "grow" | "shrink" => {
                let d = num(rest, verb)?;
                if d <= 0.0 || d > 500.0 {
                    return Err(format!("{verb}: use a distance above 0 and up to 500"));
                }
                Ok(Action::Grow(if verb.eq_ignore_ascii_case("shrink") { -d } else { d }))
            }
            "slab" => {
                let h = num(rest, "slab")?;
                if h < 0.0 || h > 500.0 {
                    return Err("slab: use a height from 0 to 500".into());
                }
                Ok(Action::Slab(h))
            }
            "" => Err("Add an action after =>".into()),
            other => Err(format!("`{other}` is not an action (count, replace, add, remove, delete, grow, shrink, slab)")),
        }
    }
}

/// Where + action.
#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    pub query: Query,
    pub action: Action,
}

impl Rule {
    pub fn parse(text: &str) -> Result<Rule, String> {
        let (left, right) = text.split_once("=>").ok_or("Write the rule as: <where> => <action>")?;
        if right.contains("=>") {
            return Err("A rule has one =>".into());
        }
        if left.trim().is_empty() {
            return Err("Add a condition before =>".into());
        }
        let query = Query::parse_strict(left)?;
        for t in &query.tokens {
            let word = match t {
                Token::Npc(..) => "npc:",
                Token::NpcType(_) => "npctype:",
                Token::Portal(_) => "portal:",
                Token::Region(_) => "region:",
                _ => continue,
            };
            return Err(format!("{word} does not describe a monster area"));
        }
        Ok(Rule { query, action: Action::parse(right)? })
    }

    /// The rule as it is typed; parsing it again gives the same rule.
    pub fn text(&self) -> String {
        format!("{} => {}", self.query.text(), self.action.text())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Opts {
    /// Counts may go above 8 (`area-count-high` in Problems).
    pub allow_above_8: bool,
}

/// What the rule does to one matched area.
#[derive(Clone, Debug, PartialEq)]
pub struct AreaPlan {
    pub index: usize,
    pub before: MobArea,
    /// The area as it will be; `None` = deleted.
    pub after: Option<MobArea>,
    pub before_text: String,
    pub after_text: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MapPlan {
    pub id: Option<u32>,
    /// The monster areas the query matched (indices, for the outline).
    pub matched: Vec<usize>,
    /// The matched areas the action changes.
    pub areas: Vec<AreaPlan>,
    /// Entries changed, removed or added, over all areas.
    pub entries: usize,
    /// Monsters on the whole map before and after.
    pub before: u64,
    pub after: u64,
    /// Entries held back at the cap.
    pub capped: usize,
    /// Areas left with no entries.
    pub emptied: usize,
    pub deleted: usize,
}

impl MapPlan {
    /// The edit ops of one undo step: replaces first, then removes from the highest index down.
    pub fn ops(&self) -> Vec<Op> {
        let mut ops = Vec::new();
        for a in &self.areas {
            if let Some(after) = &a.after {
                ops.push(Op::replace(a.index, Rec::Mob(a.before.clone()), Rec::Mob(after.clone())));
            }
        }
        let mut removes: Vec<&AreaPlan> = self.areas.iter().filter(|a| a.after.is_none()).collect();
        removes.sort_by_key(|a| std::cmp::Reverse(a.index));
        for a in removes {
            ops.push(Op::remove(a.index, Rec::Mob(a.before.clone())));
        }
        ops
    }
}

/// What a rule does on every map it matches in.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    pub per_map: Vec<MapPlan>,
}

impl Plan {
    pub fn matched(&self) -> usize {
        self.per_map.iter().map(|m| m.matched.len()).sum()
    }

    pub fn changed(&self) -> usize {
        self.per_map.iter().map(|m| m.areas.len()).sum()
    }

    pub fn entries(&self) -> usize {
        self.per_map.iter().map(|m| m.entries).sum()
    }

    pub fn capped(&self) -> usize {
        self.per_map.iter().map(|m| m.capped).sum()
    }

    pub fn emptied(&self) -> usize {
        self.per_map.iter().map(|m| m.emptied).sum()
    }

    /// Maps the rule changes something on.
    pub fn maps_changed(&self) -> usize {
        self.per_map.iter().filter(|m| !m.areas.is_empty()).count()
    }
}

/// "Wolf x2, Boar x1" (with `shape`, also the box size: geometry actions change nothing else).
pub fn area_text(env: &Env, a: &MobArea, shape: bool) -> String {
    let mut t = if a.mobs.is_empty() { "(no monsters)".to_string() } else { a.mobs.iter().map(|e| format!("{} x{}", env.mob_name(e.mob), e.count)).collect::<Vec<_>>().join(", ") };
    if shape {
        let (w, d, h) = ((a.max[0] - a.min[0]).abs(), (a.max[2] - a.min[2]).abs(), (a.max[1] - a.min[1]).abs());
        t.push_str(&format!(" · {w:.1} x {d:.1}, height {h:.1}"));
    }
    t
}

struct Outcome {
    changed: bool,
    after: Option<MobArea>,
    entries: usize,
    capped: usize,
}

fn count_action(a: &mut MobArea, op: CountOp, targets: &[u32], opts: Opts) -> (usize, usize) {
    let (mut entries, mut capped) = (0, 0);
    let mut keep: Vec<MobEntry> = Vec::with_capacity(a.mobs.len());
    for e in a.mobs.drain(..) {
        if !targets.is_empty() && !targets.contains(&e.mob) {
            keep.push(e);
            continue;
        }
        let new: u64 = match op {
            CountOp::Add(n) => e.count as u64 + n as u64,
            CountOp::Sub(n) => (e.count as u64).saturating_sub(n as u64),
            CountOp::Set(n) => n as u64,
            CountOp::Mul(f) => ((e.count as f64 * f as f64) + 0.5).floor().clamp(0.0, u32::MAX as f64) as u64,
        };
        if new == 0 {
            entries += 1; // the entry goes
            continue;
        }
        let limit = if opts.allow_above_8 { u32::MAX as u64 } else { MAX_COUNT.max(e.count) as u64 };
        let n = if new > limit {
            capped += 1;
            limit
        } else {
            new
        };
        if n != e.count as u64 {
            entries += 1;
        }
        keep.push(MobEntry { mob: e.mob, count: n as u32 });
    }
    a.mobs = keep;
    (entries, capped)
}

/// The rule applied to one area (`changed` is false when nothing changes; a count held back at the cap is still counted).
fn apply(rule: &Rule, a: &MobArea, targets: &[u32], opts: Opts) -> Outcome {
    let mut b = a.clone();
    let (mut entries, mut capped) = (0usize, 0usize);
    match &rule.action {
        Action::Count(op) => (entries, capped) = count_action(&mut b, *op, targets, opts),
        Action::Replace { from, to } => {
            entries = a.mobs.iter().filter(|e| e.mob == *from).count();
            edit::mob_replace(&mut b, *from, *to);
        }
        Action::Add { mob, count } => {
            let limit = |old: u32| if opts.allow_above_8 { u32::MAX } else { MAX_COUNT.max(old) };
            match b.mobs.iter_mut().find(|e| e.mob == *mob) {
                Some(e) => {
                    let new = e.count.saturating_add(*count);
                    let n = new.min(limit(e.count));
                    capped += (n < new) as usize;
                    entries += (n != e.count) as usize;
                    e.count = n;
                }
                None => {
                    let n = (*count).min(limit(0));
                    capped += (n < *count) as usize;
                    b.mobs.push(MobEntry { mob: *mob, count: n });
                    entries += 1;
                }
            }
        }
        Action::Remove(m) => {
            entries = a.mobs.iter().filter(|e| e.mob == *m).count();
            edit::mob_remove(&mut b, *m);
        }
        Action::Delete => return Outcome { changed: true, after: None, entries: 0, capped: 0 },
        Action::Grow(d) => edit::mob_grow(&mut b, *d),
        Action::Slab(h) => edit::mob_set_height(&mut b, *h),
    }
    Outcome { changed: b != *a, after: Some(b), entries, capped }
}

/// What the rule does on one map, given the areas the query matched there (indices into `map.mob_areas`).
pub fn plan_map(rule: &Rule, id: Option<u32>, map: &shaiya_svmap::Svmap, env: &Env, opts: Opts, matched: &[usize]) -> MapPlan {
    let targets = if matches!(rule.action, Action::Count(_)) { rule.query.mobs() } else { Vec::new() };
    let total = |a: &MobArea| a.mobs.iter().map(|e| e.count as u64).sum::<u64>();
    let mut p = MapPlan { id, before: map.mob_areas.iter().map(total).sum(), ..Default::default() };
    p.after = p.before;
    let shape = matches!(rule.action, Action::Grow(_) | Action::Slab(_));
    for &i in matched {
        let Some(a) = map.mob_areas.get(i) else { continue };
        p.matched.push(i);
        let o = apply(rule, a, &targets, opts);
        p.capped += o.capped;
        if !o.changed {
            continue;
        }
        p.entries += o.entries;
        p.after = p.after - total(a) + o.after.as_ref().map_or(0, total);
        match &o.after {
            None => p.deleted += 1,
            Some(b) => {
                if b.mobs.is_empty() && !a.mobs.is_empty() {
                    p.emptied += 1;
                }
            }
        }
        let after_text = o.after.as_ref().map_or("deleted".to_string(), |b| area_text(env, b, shape));
        p.areas.push(AreaPlan { index: i, before: a.clone(), after: o.after, before_text: area_text(env, a, shape), after_text });
    }
    p
}

/// What a rule does on every map: the areas the query matches are found with Find's search (open tabs live), then the action is
/// worked out per area. A map with no match has no row. `allow` limits the areas to a given set (Replace monster from Find).
pub fn plan(rule: &Rule, maps: &[MapIn], env: &Env, opts: Opts, allow: Option<&HashSet<(Option<u32>, usize)>>) -> Plan {
    let hits = search::search_sections(&rule.query, maps, env, usize::MAX, &[SectionId::MobAreas]);
    let mut by_map: std::collections::HashMap<Option<u32>, Vec<usize>> = std::collections::HashMap::new();
    for h in hits {
        if allow.is_none_or(|s| s.contains(&(h.map, h.index))) {
            by_map.entry(h.map).or_default().push(h.index);
        }
    }
    let mut out = Plan::default();
    for mi in maps {
        let Some(mut idx) = by_map.remove(&mi.id) else { continue };
        idx.sort_unstable();
        idx.dedup();
        if !idx.is_empty() {
            out.per_map.push(plan_map(rule, mi.id, mi.map, env, opts, &idx));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use shaiya_svmap::Svmap;

    fn area(x: f32, mobs: &[(u32, u32)]) -> MobArea {
        MobArea { min: [x, 0.0, x], max: [x + 4.0, 2.0, x + 4.0], mobs: mobs.iter().map(|(m, c)| MobEntry { mob: *m, count: *c }).collect() }
    }

    fn map_with(areas: Vec<MobArea>) -> Svmap {
        let mut m = Svmap::new(128);
        m.mob_areas = areas;
        m
    }

    fn with_env<R>(f: impl FnOnce(&Env) -> R) -> R {
        let mob = |id: u32| Some((format!("Mob{id}"), id));
        let npc = |_: u32, _: u32| None;
        let name = |id: u32| format!("Map{id}");
        let region = |_: u32, _: usize| None;
        f(&Env { mob: &mob, npc: &npc, map_name: &name, region: &region })
    }

    #[test]
    fn bulk_rule_parse_roundtrip() {
        for text in [
            "map:1-5 mob:12 => count +1",
            "map:1-5,35 => count -2",
            "mob:5 count:2-3 => count = 4",
            "mob:5 => count * 1.5",
            "mob:12 => replace 12 with 14",
            "map:3 lv:10-20 => add 7 *2",
            "mob:9 => remove 9",
            "map:7 count:1 => delete",
            "mob:5 => grow 1.5",
            "mob:5 => shrink 2",
            "map:0-76 => slab 2",
            "wolf code:area-count-high => count = 8",
            "lv:12 => count +1",
            "5 => count +1",
        ] {
            let r = Rule::parse(text).unwrap_or_else(|e| panic!("{text}: {e}"));
            assert_eq!(r.text(), text, "round trip");
            assert_eq!(Rule::parse(&r.text()).unwrap(), r);
        }
        // spaces and case are forgiven, the printed form is the canonical one
        assert_eq!(Rule::parse("  MAP:1-5   mob:12=>count   + 1 ").unwrap().text(), "map:1-5 mob:12 => count +1");
        assert_eq!(Rule::parse("mob:5 => count*2").unwrap().text(), "mob:5 => count * 2");
        // every bad rule gives a message that says what to fix
        for (text, want) in [
            ("mob:5", "<where> => <action>"),
            ("=> delete", "condition"),
            ("mob:5 =>", "action"),
            ("mob:5 => nonsense", "not an action"),
            ("mob:x => delete", "does not fit mob:"),
            ("map:5-1 => delete", "does not fit map:"),
            ("foo:1 => delete", "no such word"),
            ("npc:1/5 => delete", "monster area"),
            ("mob:5 => count", "count needs"),
            ("mob:5 => count +x", "not a whole number"),
            ("mob:5 => count +0", "1 or more"),
            ("mob:5 => count = 0", "remove"),
            ("mob:5 => count * -1", "above 0"),
            ("mob:5 => replace 5 with 5", "different"),
            ("mob:5 => replace 5 to 6", "replace needs"),
            ("mob:5 => add 5", "add needs"),
            ("mob:5 => add 5 *0", "1 or more"),
            ("mob:5 => remove", "not a whole number"),
            ("mob:5 => delete now", "nothing after"),
            ("mob:5 => grow 0", "distance"),
            ("mob:5 => slab -1", "height"),
            ("mob:5 => delete => delete", "one =>"),
            // a character of two bytes across byte 5 (review: this panicked)
            ("mob:5 => abcdéx", "not an action"),
            ("mob:5 => countä", "count needs"),
        ] {
            let e = Rule::parse(text).expect_err(text);
            assert!(e.contains(want), "{text}: `{e}` should mention `{want}`");
        }
    }

    #[test]
    fn bulk_plan_counts_and_ops() {
        // 3 maps with monster 5 in three areas each (one already at 8); the rule covers maps 0 and 1
        let mk = |extra: u32| map_with(vec![area(0.0, &[(5, 2), (6, 1)]), area(10.0, &[(7, 3)]), area(20.0, &[(5, 8)]), area(30.0, &[(5, extra)])]);
        let (m0, m1, m2) = (mk(3), mk(1), mk(1));
        let ins = [MapIn { id: Some(0), map: &m0, findings: None }, MapIn { id: Some(1), map: &m1, findings: None }, MapIn { id: Some(2), map: &m2, findings: None }];
        with_env(|env| {
            let rule = Rule::parse("map:0-1 mob:5 => count +1").unwrap();
            let p = plan(&rule, &ins, env, Opts::default(), None);
            assert_eq!(p.per_map.len(), 2, "map 2 is not matched");
            assert_eq!(p.matched(), 6);
            for mp in &p.per_map {
                assert_eq!(mp.matched, vec![0, 2, 3]);
                // the area at 8 is held at 8 (capped), the other two grow
                assert_eq!((mp.capped, mp.areas.len(), mp.entries), (1, 2, 2));
                assert_eq!(mp.areas.iter().map(|a| a.index).collect::<Vec<_>>(), vec![0, 3]);
                assert_eq!(mp.areas[0].after_text, "Mob5 x3, Mob6 x1", "only monster 5 changes");
            }
            assert_eq!((p.per_map[0].before, p.per_map[0].after), (2 + 1 + 3 + 8 + 3, 3 + 1 + 3 + 8 + 4));
            assert_eq!(p.capped(), 2);
            // applying the ops of one map is one undo step that undoes exactly
            let mut m = m0.clone();
            let ops = p.per_map[0].ops();
            for o in &ops {
                o.apply(&mut m);
            }
            assert_eq!(m.mob_areas[0].mobs.iter().map(|e| (e.mob, e.count)).collect::<Vec<_>>(), vec![(5, 3), (6, 1)]);
            assert_eq!(m.mob_areas[1], m0.mob_areas[1]);
            assert_eq!(m.mob_areas[2].mobs[0].count, 8);
            assert_eq!(m.mob_areas[3].mobs[0].count, 4);
            for o in ops.iter().rev() {
                o.invert().apply(&mut m);
            }
            assert_eq!(m, m0);
            // above 8 allowed: the area at 8 grows too
            let p2 = plan(&rule, &ins, env, Opts { allow_above_8: true }, None);
            assert_eq!((p2.capped(), p2.changed()), (0, 6));
        });
    }

    #[test]
    fn every_action_does_what_it_says() {
        let m = map_with(vec![area(0.0, &[(5, 2), (6, 4)]), area(10.0, &[(5, 1)]), area(20.0, &[(7, 1)])]);
        let ins = [MapIn { id: Some(1), map: &m, findings: None }];
        let run = |text: &str| -> (Plan, Svmap) {
            with_env(|env| {
                let p = plan(&Rule::parse(text).unwrap(), &ins, env, Opts::default(), None);
                let mut n = m.clone();
                for mp in &p.per_map {
                    for o in mp.ops() {
                        o.apply(&mut n);
                    }
                }
                (p, n)
            })
        };
        let counts = |n: &Svmap, i: usize| n.mob_areas[i].mobs.iter().map(|e| (e.mob, e.count)).collect::<Vec<_>>();
        // a count action without mob: words touches every entry
        let (_, n) = run("map:1 count:1-6 => count +1");
        assert_eq!(counts(&n, 0), vec![(5, 3), (6, 5)]);
        // a count that drops below 1 removes the entry; the area stays, empty
        let (p, n) = run("mob:5 count:1 => count -1");
        assert_eq!(n.mob_areas.len(), 3);
        assert!(n.mob_areas[1].mobs.is_empty());
        assert_eq!((p.emptied(), p.entries()), (1, 1));
        // set, multiply (half up), replace, add, remove
        assert_eq!(counts(&run("mob:5 => count = 7").1, 0), vec![(5, 7), (6, 4)]);
        assert_eq!(counts(&run("mob:6 => count * 1.5").1, 0), vec![(5, 2), (6, 6)]);
        assert_eq!(counts(&run("mob:5 => count * 1.25").1, 0), vec![(5, 3), (6, 4)], "2.5 rounds up");
        assert_eq!(counts(&run("mob:5 => replace 5 with 9").1, 1), vec![(9, 1)]);
        assert_eq!(counts(&run("mob:7 => add 8 *2").1, 2), vec![(7, 1), (8, 2)]);
        assert_eq!(counts(&run("mob:7 => add 7 *2").1, 2), vec![(7, 3)], "a monster that is there gets the count added");
        assert_eq!(counts(&run("mob:5 => remove 5").1, 0), vec![(6, 4)]);
        // delete removes whole areas, highest index first, and undo puts them back
        let (p, n) = run("mob:5 => delete");
        assert_eq!(n.mob_areas.len(), 1);
        assert_eq!(p.per_map[0].deleted, 2);
        let mut back = n.clone();
        for o in p.per_map[0].ops().iter().rev() {
            o.invert().apply(&mut back);
        }
        assert_eq!(back, m);
        // grow and slab
        let (_, n) = run("mob:7 => grow 1");
        assert_eq!((n.mob_areas[2].min[0], n.mob_areas[2].max[0]), (19.0, 25.0));
        let (_, n) = run("mob:7 => slab 5");
        assert_eq!(n.mob_areas[2].max[1] - n.mob_areas[2].min[1], 5.0);
        // a rule that changes nothing leaves no area row but still counts the match
        let (p, _) = run("mob:7 => replace 5 with 9");
        assert_eq!((p.matched(), p.changed()), (1, 0));
    }
}
