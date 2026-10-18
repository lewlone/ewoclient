//! Survival block breaking (M217): `BlockBehaviour.getDestroyProgress`,
//! `Player.getDestroySpeed` and `MultiPlayerGameMode`'s start / continue /
//! stop machine, free of any socket so it can be tested.
//!
//! See `docs/rewo/M217-survival-mining-plan.md` for the decompile excerpts.
//! The app feeds the machine one call per client tick while the attack key is
//! held on a block and sends what it returns.

/// Everything `getDestroyProgress` reads for one block and the local player.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MiningInputs {
    /// The block's `destroyTime` (`-1` = unbreakable).
    pub destroy_time: f32,
    /// `BlockState.requiresCorrectToolForDrops()`.
    pub requires_correct_tool: bool,
    /// The held item's `getDestroySpeed(state)`: its `tool` component's first
    /// rule with a speed whose blocks match, else `defaultMiningSpeed`; 1.0
    /// with no `tool` component.
    pub tool_speed: f32,
    /// The held item's `isCorrectToolForDrops(state)` (false with no tool).
    pub tool_correct: bool,
    /// `Attributes.MINING_EFFICIENCY`, added when the tool speed is above 1.
    pub mining_efficiency: f32,
    /// `MobEffectUtil.getDigSpeedAmplification` when the player has haste or
    /// conduit power, else `None`.
    pub dig_speed_amplifier: Option<i32>,
    /// Mining fatigue's amplifier, if active.
    pub fatigue_amplifier: Option<i32>,
    /// `Attributes.BLOCK_BREAK_SPEED` (1.0 by default).
    pub block_break_speed: f32,
    /// `Attributes.SUBMERGED_MINING_SPEED` (0.2 by default), applied with the
    /// eye in water.
    pub submerged_mining_speed: f32,
    pub eye_in_water: bool,
    pub on_ground: bool,
}

/// `Player.getDestroySpeed(state)`.
pub fn player_destroy_speed(i: &MiningInputs) -> f32 {
    let mut speed = i.tool_speed;
    if speed > 1.0 {
        speed += i.mining_efficiency;
    }
    if let Some(a) = i.dig_speed_amplifier {
        speed *= 1.0 + (a + 1) as f32 * 0.2;
    }
    if let Some(a) = i.fatigue_amplifier {
        speed *= match a {
            0 => 0.3,
            1 => 0.09,
            2 => 0.0027,
            _ => 8.1E-4,
        };
    }
    speed *= i.block_break_speed;
    if i.eye_in_water {
        speed *= i.submerged_mining_speed;
    }
    if !i.on_ground {
        speed /= 5.0;
    }
    speed
}

/// `BlockBehaviour.getDestroyProgress`: the share of the block one tick of
/// mining removes.
pub fn destroy_progress(i: &MiningInputs) -> f32 {
    if i.destroy_time == -1.0 {
        return 0.0;
    }
    let correct = !i.requires_correct_tool || i.tool_correct;
    let modifier = if correct { 30.0 } else { 100.0 };
    player_destroy_speed(i) / i.destroy_time / modifier
}

/// What the machine asks the caller to send (`ServerboundPlayerActionPacket`
/// actions) and whether to break the block locally (vanilla's predicted
/// `destroyBlock`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MiningSend {
    /// `START_DESTROY_BLOCK` at `(pos, face)`; `broke` = the block breaks now
    /// (creative, or a survival instabreak).
    Start {
        pos: [i32; 3],
        face: u8,
        broke: bool,
    },
    /// `STOP_DESTROY_BLOCK` — the progress reached 1; the block breaks.
    Stop { pos: [i32; 3], face: u8 },
    /// `ABORT_DESTROY_BLOCK` for the previous target.
    Abort { pos: [i32; 3] },
}

/// The block under the crosshair as the machine needs it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Target {
    pub pos: [i32; 3],
    pub face: u8,
    pub air: bool,
    /// [`destroy_progress`] for this block with the current inputs.
    pub progress: f32,
}

/// `MultiPlayerGameMode`'s destroy fields. `item` is the main-hand stack's
/// identity (`ItemStack.isSameItemSameComponents` — item id and component
/// digest): switching items restarts the block.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Mining {
    destroying: bool,
    pos: [i32; 3],
    item: Option<(i32, u64)>,
    progress: f32,
    ticks: f32,
    delay: i32,
}

impl Mining {
    fn same_target(&self, pos: [i32; 3], item: Option<(i32, u64)>) -> bool {
        self.pos == pos && self.item == item
    }

    /// `startDestroyBlock` — the attack key's press on a block.
    pub fn start(
        &mut self,
        t: Target,
        creative: bool,
        item: Option<(i32, u64)>,
    ) -> Vec<MiningSend> {
        if creative {
            self.delay = 5;
            return vec![MiningSend::Start {
                pos: t.pos,
                face: t.face,
                broke: !t.air,
            }];
        }
        let mut out = Vec::new();
        if !self.destroying || !self.same_target(t.pos, item) {
            if self.destroying {
                out.push(MiningSend::Abort { pos: self.pos });
            }
            let instabreak = !t.air && t.progress >= 1.0;
            if !instabreak {
                self.destroying = true;
                self.pos = t.pos;
                self.item = item;
                self.progress = 0.0;
                self.ticks = 0.0;
            }
            out.push(MiningSend::Start {
                pos: t.pos,
                face: t.face,
                broke: instabreak,
            });
        }
        out
    }

    /// `continueDestroyBlock` — one client tick with the key held on `t`.
    /// The bool is its return value: whether the caller swings this tick.
    pub fn tick(
        &mut self,
        t: Target,
        creative: bool,
        item: Option<(i32, u64)>,
    ) -> (Vec<MiningSend>, bool) {
        if self.delay > 0 {
            self.delay -= 1;
            return (Vec::new(), true);
        }
        if creative {
            self.delay = 5;
            return (
                vec![MiningSend::Start {
                    pos: t.pos,
                    face: t.face,
                    broke: !t.air,
                }],
                true,
            );
        }
        if self.same_target(t.pos, item) {
            if t.air {
                self.destroying = false;
                return (Vec::new(), false);
            }
            self.progress += t.progress;
            self.ticks += 1.0;
            if self.progress >= 1.0 {
                self.destroying = false;
                self.progress = 0.0;
                self.ticks = 0.0;
                self.delay = 5;
                return (
                    vec![MiningSend::Stop {
                        pos: t.pos,
                        face: t.face,
                    }],
                    true,
                );
            }
            return (Vec::new(), true);
        }
        (self.start(t, creative, item), true)
    }

    /// `stopDestroyBlock` — the key released, or the crosshair left blocks.
    pub fn stop(&mut self) -> Option<MiningSend> {
        if !self.destroying {
            return None;
        }
        self.destroying = false;
        self.progress = 0.0;
        Some(MiningSend::Abort { pos: self.pos })
    }

    /// `getDestroyStage`: the crack overlay's stage, -1 for none.
    pub fn stage(&self) -> i32 {
        if self.progress > 0.0 {
            (self.progress * 10.0) as i32
        } else {
            -1
        }
    }

    pub fn destroying(&self) -> bool {
        self.destroying
    }

    pub fn target(&self) -> Option<[i32; 3]> {
        self.destroying.then_some(self.pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hand(destroy_time: f32, requires: bool) -> MiningInputs {
        MiningInputs {
            destroy_time,
            requires_correct_tool: requires,
            tool_speed: 1.0,
            tool_correct: false,
            mining_efficiency: 0.0,
            dig_speed_amplifier: None,
            fatigue_amplifier: None,
            block_break_speed: 1.0,
            submerged_mining_speed: 0.2,
            eye_in_water: false,
            on_ground: true,
        }
    }

    /// The vanilla numbers, by hand: dirt (0.5, no tool needed) is `1 / 0.5 /
    /// 30` a tick, 15 ticks; stone by hand (1.5, needs a pickaxe) is `1 / 1.5 /
    /// 100`; with a wooden pickaxe (speed 2, correct) `2 / 1.5 / 30`;
    /// bedrock never.
    #[test]
    fn destroy_progress_is_vanillas() {
        assert!((destroy_progress(&hand(0.5, false)) - 1.0 / 0.5 / 30.0).abs() < 1e-7);
        assert!((destroy_progress(&hand(1.5, true)) - 1.0 / 1.5 / 100.0).abs() < 1e-7);
        let wood = MiningInputs {
            tool_speed: 2.0,
            tool_correct: true,
            ..hand(1.5, true)
        };
        assert!((destroy_progress(&wood) - 2.0 / 1.5 / 30.0).abs() < 1e-7);
        assert_eq!(destroy_progress(&hand(-1.0, false)), 0.0);
        // Off the ground a fifth; efficiency only above speed 1; haste II +60%.
        let air = MiningInputs {
            on_ground: false,
            ..hand(0.5, false)
        };
        assert!((destroy_progress(&air) - destroy_progress(&hand(0.5, false)) / 5.0).abs() < 1e-7);
        let eff = MiningInputs {
            mining_efficiency: 26.0,
            ..hand(0.5, false)
        };
        assert_eq!(player_destroy_speed(&eff), 1.0);
        let haste = MiningInputs {
            dig_speed_amplifier: Some(1),
            ..hand(0.5, false)
        };
        assert!((player_destroy_speed(&haste) - 1.4).abs() < 1e-6);
    }

    fn dirt(pos: [i32; 3]) -> Target {
        Target {
            pos,
            face: 1,
            air: false,
            progress: destroy_progress(&hand(0.5, false)),
        }
    }

    /// Dirt by hand: START on the press, 15 ticks of progress, STOP on the
    /// tick it reaches 1 (float addition of 1/15 reaches it on the 15th),
    /// then the five-tick delay.
    #[test]
    fn survival_dirt_takes_fifteen_ticks_then_stops() {
        let mut m = Mining::default();
        let p = [1, 64, 2];
        assert_eq!(
            m.start(dirt(p), false, None),
            vec![MiningSend::Start {
                pos: p,
                face: 1,
                broke: false
            }]
        );
        let mut stopped_at = None;
        for tick in 1..=20 {
            let (out, _) = m.tick(dirt(p), false, None);
            if out.contains(&MiningSend::Stop { pos: p, face: 1 }) {
                stopped_at = Some(tick);
                break;
            }
        }
        assert_eq!(stopped_at, Some(15));
        assert!(!m.destroying());
        // The delay: the next five ticks do nothing.
        for _ in 0..5 {
            assert_eq!(m.tick(dirt(p), false, None).0, Vec::new());
        }
    }

    /// Moving to another block aborts the old one and starts the new; a new
    /// item restarts too; releasing aborts; an instabreak sends START only.
    #[test]
    fn retargeting_aborts_and_release_aborts() {
        let mut m = Mining::default();
        let (a, b) = ([0, 64, 0], [1, 64, 0]);
        m.start(dirt(a), false, None);
        m.tick(dirt(a), false, None);
        let (out, _) = m.tick(dirt(b), false, None);
        assert_eq!(
            out,
            vec![
                MiningSend::Abort { pos: a },
                MiningSend::Start {
                    pos: b,
                    face: 1,
                    broke: false
                }
            ]
        );
        let (out, _) = m.tick(dirt(b), false, Some((5, 0)));
        assert_eq!(
            out,
            vec![
                MiningSend::Abort { pos: b },
                MiningSend::Start {
                    pos: b,
                    face: 1,
                    broke: false
                }
            ]
        );
        assert_eq!(m.stop(), Some(MiningSend::Abort { pos: b }));
        assert_eq!(m.stop(), None);
        let grass = Target {
            pos: a,
            face: 1,
            air: false,
            progress: 1.0,
        };
        assert_eq!(
            m.start(grass, false, None),
            vec![MiningSend::Start {
                pos: a,
                face: 1,
                broke: true
            }]
        );
        assert!(!m.destroying());
    }

    /// Creative: every START breaks, and holding repeats every six ticks
    /// (`destroyDelay = 5`, then the sixth tick starts again).
    #[test]
    fn creative_breaks_on_start_and_repeats_after_the_delay() {
        let mut m = Mining::default();
        let p = [0, 64, 0];
        assert_eq!(
            m.start(dirt(p), true, None),
            vec![MiningSend::Start {
                pos: p,
                face: 1,
                broke: true
            }]
        );
        let sends: Vec<usize> = (1..=12)
            .filter(|_| !m.tick(dirt(p), true, None).0.is_empty())
            .collect();
        assert_eq!(sends.len(), 2, "ticks 6 and 12");
    }
}
