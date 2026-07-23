//! OptiFine CEM **animation expression language** — the interpreter behind
//! `_animations.jpm` (REWO_PLAN §12 M9c). Fresh Animations drives every
//! bone through expressions like `clamp(head_yaw,-90,90)` and
//! `var.r +age/35*pi*2`; this module lexes + parses one expression string
//! into an AST once (at pack load) and evaluates it per frame against an
//! [`AnimContext`] (built-in inputs + user `var.*` slots).
//!
//! Scope: the operators + functions Fresh Animations actually uses (surveyed
//! from the packs) — arithmetic, comparison, boolean, `sin/cos/clamp/…`, and
//! the variadic `if(c,a,b,…)`. Values are `f32`; booleans are 0/1.

use std::collections::HashMap;

// ---------------------------------------------------------------------------
// AST
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
enum Bin {
    Add, Sub, Mul, Div, Rem,
    Eq, Ne, Lt, Gt, Le, Ge,
    And, Or,
}

#[derive(Debug, Clone)]
enum Expr {
    Num(f32),
    /// Built-in input (resolved to a `Var` id at parse time — no per-frame
    /// string compare).
    Builtin(Var),
    /// User variable (`var.name` / `varb.name`), resolved to a slot index.
    User(usize),
    Neg(Box<Expr>),
    Not(Box<Expr>),
    BinOp(Bin, Box<Expr>, Box<Expr>),
    Call(Func, Vec<Expr>),
}

/// Built-in animation inputs (OptiFine's standard variable set — the subset
/// Fresh Animations references).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Var {
    HeadYaw, HeadPitch, LimbSwing, LimbSpeed, Age, Time, FrameTime, FrameCounter,
    IsOnGround, IsInWater, IsRiding, IsChild, IsAggressive, IsHurt, IsAlive,
    IsSneaking, IsSprinting, IsBurning, HurtTime, DeathTime, Health, MaxHealth,
    Id, PosX, PosY, PosZ, PlayerPosX, PlayerPosY, PlayerPosZ, RotY,
    IsRidden, IsSitting, IsInLava, IsTamed, IsOnShoulder, SwingProgress, Pi,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Func {
    Sin, Cos, Tan, Asin, Acos, Atan, Atan2, ToRad, ToDeg,
    Min, Max, Clamp, Abs, Floor, Ceil, Round, Exp, Frac, Log, Pow, Sqrt, Sign, Fmod,
    Random, If, Between, Equals, In,
}

// ---------------------------------------------------------------------------
// Evaluation context
// ---------------------------------------------------------------------------

/// Per-frame inputs + user-variable storage. The animation runtime fills the
/// built-in fields from entity state each frame; `user` holds the `var.*`
/// slots (assigned in file order, read later in the same frame).
#[derive(Default)]
pub struct AnimContext {
    pub head_yaw: f32,
    pub head_pitch: f32,
    pub limb_swing: f32,
    pub limb_speed: f32,
    pub age: f32,
    pub time: f32,
    pub frame_time: f32,
    pub frame_counter: f32,
    pub is_on_ground: bool,
    pub is_in_water: bool,
    pub is_riding: bool,
    pub is_child: bool,
    pub is_aggressive: bool,
    pub is_hurt: bool,
    pub is_alive: bool,
    pub is_sneaking: bool,
    pub is_sprinting: bool,
    pub is_burning: bool,
    pub hurt_time: f32,
    pub death_time: f32,
    pub health: f32,
    pub max_health: f32,
    pub id: f32,
    pub pos_x: f32,
    pub pos_y: f32,
    pub pos_z: f32,
    pub player_pos_x: f32,
    pub player_pos_y: f32,
    pub player_pos_z: f32,
    /// Entity body yaw in **radians** (OptiFine `rot_y`). FA derives its
    /// turn-detection vars (`var.tr`/`var.tp`/`var.tq`) from it.
    pub rot_y: f32,
    pub is_ridden: bool,
    pub is_sitting: bool,
    pub is_in_lava: bool,
    pub is_tamed: bool,
    pub is_on_shoulder: bool,
    pub swing_progress: f32,
    /// User `var.*` slots (indexed by the program's slot table).
    pub user: Vec<f32>,
}

impl AnimContext {
    fn builtin(&self, v: Var) -> f32 {
        let b = |x: bool| if x { 1.0 } else { 0.0 };
        match v {
            Var::HeadYaw => self.head_yaw,
            Var::HeadPitch => self.head_pitch,
            Var::LimbSwing => self.limb_swing,
            Var::LimbSpeed => self.limb_speed,
            Var::Age => self.age,
            Var::Time => self.time,
            Var::FrameTime => self.frame_time,
            Var::FrameCounter => self.frame_counter,
            Var::IsOnGround => b(self.is_on_ground),
            Var::IsInWater => b(self.is_in_water),
            Var::IsRiding => b(self.is_riding),
            Var::IsChild => b(self.is_child),
            Var::IsAggressive => b(self.is_aggressive),
            Var::IsHurt => b(self.is_hurt),
            Var::IsAlive => b(self.is_alive),
            Var::IsSneaking => b(self.is_sneaking),
            Var::IsSprinting => b(self.is_sprinting),
            Var::IsBurning => b(self.is_burning),
            Var::HurtTime => self.hurt_time,
            Var::DeathTime => self.death_time,
            Var::Health => self.health,
            Var::MaxHealth => self.max_health,
            Var::Id => self.id,
            Var::PosX => self.pos_x,
            Var::PosY => self.pos_y,
            Var::PosZ => self.pos_z,
            Var::PlayerPosX => self.player_pos_x,
            Var::PlayerPosY => self.player_pos_y,
            Var::PlayerPosZ => self.player_pos_z,
            Var::RotY => self.rot_y,
            Var::IsRidden => b(self.is_ridden),
            Var::IsSitting => b(self.is_sitting),
            Var::IsInLava => b(self.is_in_lava),
            Var::IsTamed => b(self.is_tamed),
            Var::IsOnShoulder => b(self.is_on_shoulder),
            Var::SwingProgress => self.swing_progress,
            Var::Pi => std::f32::consts::PI,
        }
    }
}

fn builtin_var(name: &str) -> Option<Var> {
    Some(match name {
        "head_yaw" => Var::HeadYaw,
        "head_pitch" => Var::HeadPitch,
        "limb_swing" => Var::LimbSwing,
        "limb_speed" => Var::LimbSpeed,
        "age" => Var::Age,
        "time" => Var::Time,
        "frame_time" => Var::FrameTime,
        "frame_counter" => Var::FrameCounter,
        "is_on_ground" => Var::IsOnGround,
        "is_in_water" => Var::IsInWater,
        "is_riding" => Var::IsRiding,
        "is_child" | "is_baby" => Var::IsChild,
        "is_aggressive" => Var::IsAggressive,
        "is_hurt" => Var::IsHurt,
        "is_alive" => Var::IsAlive,
        "is_sneaking" => Var::IsSneaking,
        "is_sprinting" => Var::IsSprinting,
        "is_burning" | "is_on_fire" => Var::IsBurning,
        "hurt_time" => Var::HurtTime,
        "death_time" => Var::DeathTime,
        "health" => Var::Health,
        "max_health" => Var::MaxHealth,
        "id" => Var::Id,
        "pos_x" => Var::PosX,
        "pos_y" => Var::PosY,
        "pos_z" => Var::PosZ,
        "player_pos_x" => Var::PlayerPosX,
        "player_pos_y" => Var::PlayerPosY,
        "player_pos_z" => Var::PlayerPosZ,
        "rot_y" => Var::RotY,
        "is_ridden" => Var::IsRidden,
        "is_sitting" => Var::IsSitting,
        "is_in_lava" => Var::IsInLava,
        "is_tamed" => Var::IsTamed,
        "is_on_shoulder" => Var::IsOnShoulder,
        "swing_progress" | "limb_swing_amount" => Var::SwingProgress,
        "pi" => Var::Pi,
        _ => return None,
    })
}

fn func_of(name: &str) -> Option<Func> {
    Some(match name {
        "sin" => Func::Sin, "cos" => Func::Cos, "tan" => Func::Tan,
        "asin" => Func::Asin, "acos" => Func::Acos, "atan" => Func::Atan, "atan2" => Func::Atan2,
        "torad" => Func::ToRad, "todeg" => Func::ToDeg,
        "min" => Func::Min, "max" => Func::Max, "clamp" => Func::Clamp,
        "abs" => Func::Abs, "floor" => Func::Floor, "ceil" => Func::Ceil, "round" => Func::Round,
        "exp" => Func::Exp, "frac" => Func::Frac, "log" => Func::Log, "pow" => Func::Pow,
        "sqrt" => Func::Sqrt, "signum" | "sign" => Func::Sign, "fmod" => Func::Fmod,
        "random" => Func::Random, "if" => Func::If,
        "between" => Func::Between, "equals" => Func::Equals, "in" => Func::In,
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// A compiled program: an ordered list of assignments (user var or bone
// channel), each with its parsed expression. Slot table interns user vars.
// ---------------------------------------------------------------------------

/// Slot registry shared across a mob's whole animation program, so `var.x`
/// assigned in one line resolves to the same slot when read in another.
#[derive(Default)]
pub struct Slots {
    map: HashMap<String, usize>,
}

impl Slots {
    pub fn get_or_add(&mut self, name: &str) -> usize {
        if let Some(&i) = self.map.get(name) {
            return i;
        }
        let i = self.map.len();
        self.map.insert(name.to_string(), i);
        i
    }
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Every interned name. Anything here that isn't a `var.*`/`varb.*` user
    /// slot or a readable `bone.channel` is an identifier the interpreter
    /// doesn't implement — it silently evaluates to 0 and quietly corrupts
    /// whatever it feeds. `cem::parse_anim` uses this to warn instead.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.map.keys().map(|s| s.as_str())
    }
}

/// Which bone channel an assignment drives (OptiFine `rx`/`ry`/`rz` rotate
/// radians, `tx`/`ty`/`tz` translate model px, `sx`/`sy`/`sz` scale,
/// `visible` shows/hides the bone and its subtree).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Rx, Ry, Rz, Tx, Ty, Tz, Sx, Sy, Sz, Visible,
}

impl Channel {
    pub fn parse(s: &str) -> Option<Channel> {
        Some(match s {
            "rx" => Channel::Rx, "ry" => Channel::Ry, "rz" => Channel::Rz,
            "tx" => Channel::Tx, "ty" => Channel::Ty, "tz" => Channel::Tz,
            "sx" => Channel::Sx, "sy" => Channel::Sy, "sz" => Channel::Sz,
            // FA toggles optional geometry with this: goat horns, bee stinger,
            // bogged mushrooms, donkey chests, a sheared snow golem's face.
            "visible" => Channel::Visible,
            _ => return None,
        })
    }
}

/// One assignment in a mob's animation program: either a `var.*` slot write
/// (feeds later expressions the same frame) or a bone channel. Bone channels
/// also carry a `slot`: their evaluated value is published there each frame so
/// later expressions can *read* `bone.channel` (FA mirrors left parts off the
/// right, e.g. `"l_eye_white.sy": "r_eye_white.sy"`, and derives uniform
/// scale, e.g. `"head2.sy": "head2.sx"`).
pub enum Target {
    Var(usize),
    Bone { part: u16, channel: Channel, slot: usize },
}

/// A whole mob's animation: ordered assignments (vars first per the file
/// order, then bones) + the user-var slot count. Evaluated top-to-bottom
/// each frame — see `crate::entities` for application to the part transforms.
pub struct AnimProgram {
    pub steps: Vec<(Target, Program)>,
    pub slot_count: usize,
}

/// A parsed expression, ready to evaluate against an [`AnimContext`].
pub struct Program {
    ast: Expr,
}

impl Program {
    /// Parse one expression string. `slots` interns any `var.*` names seen.
    pub fn compile(src: &str, slots: &mut Slots) -> Result<Program, String> {
        let toks = lex(src)?;
        let mut p = Parser { toks, pos: 0, slots };
        let ast = p.parse_expr(0)?;
        if p.pos != p.toks.len() {
            return Err(format!("trailing tokens in {src:?}"));
        }
        Ok(Program { ast })
    }

    pub fn eval(&self, ctx: &AnimContext) -> f32 {
        eval(&self.ast, ctx)
    }
}

// ---------------------------------------------------------------------------
// Lexer
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f32),
    Ident(String),
    Plus, Minus, Star, Slash, Percent,
    Eq, Ne, Lt, Gt, Le, Ge,
    And, Or, Bang,
    LParen, RParen, Comma,
}

fn lex(s: &str) -> Result<Vec<Tok>, String> {
    let b = s.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < b.len() {
        let c = b[i] as char;
        match c {
            ' ' | '\t' | '\n' | '\r' => i += 1,
            '+' => { out.push(Tok::Plus); i += 1; }
            '-' => { out.push(Tok::Minus); i += 1; }
            '*' => { out.push(Tok::Star); i += 1; }
            '/' => { out.push(Tok::Slash); i += 1; }
            '%' => { out.push(Tok::Percent); i += 1; }
            '(' => { out.push(Tok::LParen); i += 1; }
            ')' => { out.push(Tok::RParen); i += 1; }
            ',' => { out.push(Tok::Comma); i += 1; }
            '=' if b.get(i + 1) == Some(&b'=') => { out.push(Tok::Eq); i += 2; }
            '!' if b.get(i + 1) == Some(&b'=') => { out.push(Tok::Ne); i += 2; }
            '!' => { out.push(Tok::Bang); i += 1; }
            '<' if b.get(i + 1) == Some(&b'=') => { out.push(Tok::Le); i += 2; }
            '>' if b.get(i + 1) == Some(&b'=') => { out.push(Tok::Ge); i += 2; }
            '<' => { out.push(Tok::Lt); i += 1; }
            '>' => { out.push(Tok::Gt); i += 1; }
            '&' if b.get(i + 1) == Some(&b'&') => { out.push(Tok::And); i += 2; }
            '|' if b.get(i + 1) == Some(&b'|') => { out.push(Tok::Or); i += 2; }
            '0'..='9' | '.' => {
                let start = i;
                while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.' || b[i] == b'e' || b[i] == b'E') {
                    i += 1;
                }
                let n: f32 = s[start..i].parse().map_err(|_| format!("bad number {:?}", &s[start..i]))?;
                out.push(Tok::Num(n));
            }
            _ if c.is_ascii_alphabetic() || c == '_' => {
                let start = i;
                while i < b.len() && {
                    let ch = b[i] as char;
                    ch.is_ascii_alphanumeric() || ch == '_' || ch == '.'
                } {
                    i += 1;
                }
                out.push(Tok::Ident(s[start..i].to_string()));
            }
            _ => return Err(format!("bad char {c:?} in {s:?}")),
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Pratt parser
// ---------------------------------------------------------------------------

struct Parser<'a> {
    toks: Vec<Tok>,
    pos: usize,
    slots: &'a mut Slots,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }
    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        self.pos += 1;
        t
    }
    fn expect(&mut self, t: &Tok) -> Result<(), String> {
        if self.peek() == Some(t) {
            self.pos += 1;
            Ok(())
        } else {
            Err(format!("expected {t:?}, got {:?}", self.peek()))
        }
    }

    /// Binding power of an infix operator (higher = tighter).
    fn infix_bp(t: &Tok) -> Option<(Bin, u8)> {
        Some(match t {
            Tok::Or => (Bin::Or, 1),
            Tok::And => (Bin::And, 2),
            Tok::Eq => (Bin::Eq, 3),
            Tok::Ne => (Bin::Ne, 3),
            Tok::Lt => (Bin::Lt, 4),
            Tok::Gt => (Bin::Gt, 4),
            Tok::Le => (Bin::Le, 4),
            Tok::Ge => (Bin::Ge, 4),
            Tok::Plus => (Bin::Add, 5),
            Tok::Minus => (Bin::Sub, 5),
            Tok::Star => (Bin::Mul, 6),
            Tok::Slash => (Bin::Div, 6),
            Tok::Percent => (Bin::Rem, 6),
            _ => return None,
        })
    }

    fn parse_expr(&mut self, min_bp: u8) -> Result<Expr, String> {
        let mut lhs = self.parse_prefix()?;
        while let Some(t) = self.peek() {
            let Some((op, bp)) = Self::infix_bp(t) else { break };
            if bp < min_bp {
                break;
            }
            self.pos += 1;
            let rhs = self.parse_expr(bp + 1)?;
            lhs = Expr::BinOp(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn parse_prefix(&mut self) -> Result<Expr, String> {
        match self.next() {
            Some(Tok::Minus) => Ok(Expr::Neg(Box::new(self.parse_expr(7)?))),
            Some(Tok::Bang) => Ok(Expr::Not(Box::new(self.parse_expr(7)?))),
            Some(Tok::Num(n)) => Ok(Expr::Num(n)),
            Some(Tok::LParen) => {
                let e = self.parse_expr(0)?;
                self.expect(&Tok::RParen)?;
                Ok(e)
            }
            Some(Tok::Ident(name)) => {
                if self.peek() == Some(&Tok::LParen) {
                    self.pos += 1;
                    let mut args = Vec::new();
                    if self.peek() != Some(&Tok::RParen) {
                        loop {
                            args.push(self.parse_expr(0)?);
                            match self.peek() {
                                Some(&Tok::Comma) => self.pos += 1,
                                _ => break,
                            }
                        }
                    }
                    self.expect(&Tok::RParen)?;
                    let f = func_of(&name).ok_or_else(|| format!("unknown function {name}"))?;
                    Ok(Expr::Call(f, args))
                } else if name == "true" {
                    Ok(Expr::Num(1.0))
                } else if name == "false" {
                    Ok(Expr::Num(0.0))
                } else if let Some(v) = builtin_var(&name) {
                    Ok(Expr::Builtin(v))
                } else {
                    // User var (var.x / varb.x) or anything else → a slot.
                    Ok(Expr::User(self.slots.get_or_add(&name)))
                }
            }
            other => Err(format!("unexpected token {other:?}")),
        }
    }
}

// ---------------------------------------------------------------------------
// Evaluator
// ---------------------------------------------------------------------------

fn eval(e: &Expr, ctx: &AnimContext) -> f32 {
    match e {
        Expr::Num(n) => *n,
        Expr::Builtin(v) => ctx.builtin(*v),
        Expr::User(i) => ctx.user.get(*i).copied().unwrap_or(0.0),
        Expr::Neg(a) => -eval(a, ctx),
        Expr::Not(a) => if eval(a, ctx) != 0.0 { 0.0 } else { 1.0 },
        Expr::BinOp(op, a, b) => {
            // Short-circuit boolean ops.
            match op {
                Bin::And => return if eval(a, ctx) != 0.0 && eval(b, ctx) != 0.0 { 1.0 } else { 0.0 },
                Bin::Or => return if eval(a, ctx) != 0.0 || eval(b, ctx) != 0.0 { 1.0 } else { 0.0 },
                _ => {}
            }
            let (x, y) = (eval(a, ctx), eval(b, ctx));
            let bf = |c: bool| if c { 1.0 } else { 0.0 };
            match op {
                Bin::Add => x + y,
                Bin::Sub => x - y,
                Bin::Mul => x * y,
                Bin::Div => x / y,
                Bin::Rem => x % y,
                Bin::Eq => bf(x == y),
                Bin::Ne => bf(x != y),
                Bin::Lt => bf(x < y),
                Bin::Gt => bf(x > y),
                Bin::Le => bf(x <= y),
                Bin::Ge => bf(x >= y),
                Bin::And | Bin::Or => unreachable!(),
            }
        }
        Expr::Call(f, a) => eval_call(*f, a, ctx),
    }
}

fn eval_call(f: Func, a: &[Expr], ctx: &AnimContext) -> f32 {
    let ev = |i: usize| a.get(i).map(|e| eval(e, ctx)).unwrap_or(0.0);
    match f {
        Func::Sin => ev(0).sin(),
        Func::Cos => ev(0).cos(),
        Func::Tan => ev(0).tan(),
        Func::Asin => ev(0).asin(),
        Func::Acos => ev(0).acos(),
        Func::Atan => ev(0).atan(),
        Func::Atan2 => ev(0).atan2(ev(1)),
        Func::ToRad => ev(0).to_radians(),
        Func::ToDeg => ev(0).to_degrees(),
        Func::Min => a.iter().map(|e| eval(e, ctx)).fold(f32::INFINITY, f32::min),
        Func::Max => a.iter().map(|e| eval(e, ctx)).fold(f32::NEG_INFINITY, f32::max),
        Func::Clamp => ev(0).clamp(ev(1), ev(2)),
        Func::Abs => ev(0).abs(),
        Func::Floor => ev(0).floor(),
        Func::Ceil => ev(0).ceil(),
        Func::Round => ev(0).round(),
        Func::Exp => ev(0).exp(),
        Func::Frac => ev(0).fract(),
        Func::Log => ev(0).ln(),
        Func::Pow => ev(0).powf(ev(1)),
        Func::Sqrt => ev(0).sqrt(),
        Func::Sign => ev(0).signum(),
        Func::Fmod => ev(0).rem_euclid(ev(1)),
        // random(seed): a stable per-seed value in [0,1). OptiFine seeds by
        // the expression; we hash the (integer) seed so `random(id)` is
        // per-entity-stable within a run.
        Func::Random => {
            let seed = ev(0).to_bits() as u64;
            let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(0x1234_5678);
            x ^= x >> 33;
            x = x.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
            x ^= x >> 33;
            (x >> 40) as f32 / (1u32 << 24) as f32
        }
        // if(c,a) / if(c,a,b) / if(c1,a1,c2,a2,…,else): pairs of cond,val
        // with an optional trailing else.
        Func::If => {
            let mut i = 0;
            while i + 1 < a.len() {
                if eval(&a[i], ctx) != 0.0 {
                    return eval(&a[i + 1], ctx);
                }
                i += 2;
            }
            if i < a.len() { eval(&a[i], ctx) } else { 0.0 }
        }
        Func::Between => {
            let v = ev(0);
            if v >= ev(1) && v <= ev(2) { 1.0 } else { 0.0 }
        }
        Func::Equals => if ev(0) == ev(1) { 1.0 } else { 0.0 },
        Func::In => {
            let v = ev(0);
            if a.iter().skip(1).any(|e| eval(e, ctx) == v) { 1.0 } else { 0.0 }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(src: &str, ctx: &AnimContext) -> f32 {
        let mut slots = Slots::default();
        Program::compile(src, &mut slots).unwrap().eval(ctx)
    }

    #[test]
    fn arithmetic_precedence() {
        let c = AnimContext::default();
        assert_eq!(run("1 + 2 * 3", &c), 7.0);
        assert_eq!(run("(1 + 2) * 3", &c), 9.0);
        assert_eq!(run("-2 * 3", &c), -6.0);
        assert!((run("10 % 3", &c) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn functions_and_pi() {
        let c = AnimContext::default();
        assert!((run("sin(pi/2)", &c) - 1.0).abs() < 1e-6);
        assert_eq!(run("clamp(150, -90, 90)", &c), 90.0);
        assert_eq!(run("min(3, max(5, 1))", &c), 3.0);
        assert!((run("torad(180)", &c) - std::f32::consts::PI).abs() < 1e-6);
    }

    #[test]
    fn builtins_and_bools() {
        let mut c = AnimContext::default();
        c.head_yaw = 120.0;
        c.is_on_ground = true;
        assert_eq!(run("clamp(head_yaw,-90,90)", &c), 90.0);
        assert_eq!(run("is_on_ground && !is_in_water", &c), 1.0);
        assert_eq!(run("if(is_on_ground, 5, 9)", &c), 5.0);
        // Variadic if: first true pair wins, else trails.
        assert_eq!(run("if(is_in_water, 1, is_on_ground, 2, 3)", &c), 2.0);
    }

    #[test]
    fn user_vars_share_slots_across_programs() {
        let mut slots = Slots::default();
        let assign = Program::compile("age * 2", &mut slots).unwrap();
        let read = Program::compile("var.x + 1", &mut slots).unwrap();
        let xslot = slots.get_or_add("var.x");
        let mut c = AnimContext::default();
        c.age = 5.0;
        c.user = vec![0.0; slots.len()];
        c.user[xslot] = assign.eval(&c); // var.x = age*2 = 10
        assert_eq!(read.eval(&c), 11.0);
    }

    /// Parse every expression from a real Fresh Animations corpus (one per
    /// line) when `REWO_EXPR_CORPUS` points at it — proves the grammar
    /// covers real FA data, not just hand-picked cases. No-op when unset.
    #[test]
    fn parses_real_fa_corpus_if_present() {
        let Ok(path) = std::env::var("REWO_EXPR_CORPUS") else { return };
        let text = std::fs::read_to_string(&path).expect("corpus");
        let (mut ok, mut fail) = (0, 0);
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let mut slots = Slots::default();
            match Program::compile(line, &mut slots) {
                Ok(_) => ok += 1,
                Err(e) => {
                    fail += 1;
                    if fail <= 5 {
                        eprintln!("PARSE FAIL: {e}\n  in: {line}");
                    }
                }
            }
        }
        eprintln!("corpus: {ok} ok, {fail} failed");
        assert_eq!(fail, 0, "{fail} FA expressions failed to parse");
    }

    #[test]
    fn random_is_stable_per_seed() {
        let c = AnimContext::default();
        let a = run("random(7)", &c);
        let b = run("random(7)", &c);
        assert_eq!(a, b);
        assert!((0.0..1.0).contains(&a));
        assert_ne!(run("random(7)", &c), run("random(8)", &c));
    }
}
