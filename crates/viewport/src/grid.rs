//! The infinite reference grid: spacing levels and their cross-fade, the fade with distance, and
//! the world segments a viewport draws. One rule for every app that draws a grid on a plane, so
//! their grids step, fade and reach alike as the view zooms.

use crate::vec::Vec3Ops;
use crate::{Camera, Vec3};

/// The origin plane a grid lies on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GridPlane {
    /// The ground: the camera frame's XY.
    #[default]
    Ground,
    /// Upright, facing a view along ±Y (the Front or Back view): the frame's XZ.
    Xz,
    /// Upright, facing a view along ±X (the Left or Right view): the frame's YZ.
    Yz,
}

impl GridPlane {
    /// The plane's two axes, in the camera's Z-up frame.
    pub fn axes(self) -> (Vec3, Vec3) {
        match self {
            GridPlane::Ground => (Vec3::X, Vec3::Y),
            GridPlane::Xz => (Vec3::X, Vec3::Z),
            GridPlane::Yz => (Vec3::Y, Vec3::Z),
        }
    }
}

/// The plane to draw the grid on for a view whose direction toward the eye is `back` (in the
/// camera's Z-up frame), given the plane the last frame drew (`last`).
///
/// The ground (frame XY) unless it is seen nearly edge-on, as in the Front or Right view; then
/// the upright origin plane facing the view, so the grid stays visible (#1470). Both thresholds
/// are further for leaving than for entering, so a view hovering at one does not flicker the
/// grid, and the upright plane changes to the other only close to a side view, never at the 45°
/// crossing (#1993).
pub fn grid_plane(back: Vec3, last: GridPlane) -> GridPlane {
    // About 11.5° above or below the ground to leave it, and 15° to return.
    const EDGE_ON: f64 = 0.2;
    const EDGE_LEAVE: f64 = 0.26;
    // About 15° from the Front/Back or Left/Right view: only that close does the wall change.
    const SIDE_VIEW: f64 = 0.966;
    let horizontal = (back.x * back.x + back.y * back.y).sqrt();
    if !back.x.is_finite() || !back.y.is_finite() || !back.z.is_finite() || !horizontal.is_finite() {
        return GridPlane::Ground;
    }
    if last == GridPlane::Ground && back.z.abs() <= EDGE_ON && horizontal > 0.0 {
        return if back.y.abs() >= back.x.abs() { GridPlane::Xz } else { GridPlane::Yz };
    }
    if last != GridPlane::Ground && back.z.abs() <= EDGE_LEAVE && horizontal > 0.0 {
        if back.x.abs() / horizontal >= SIDE_VIEW {
            return GridPlane::Yz;
        }
        if back.y.abs() / horizontal >= SIDE_VIEW {
            return GridPlane::Xz;
        }
        return last;
    }
    GridPlane::Ground
}

/// Grid spacing (minor, major) for a view of the given half height: the minor step is 1, 2 or 5
/// times a power of ten near a tenth of it, the major five minor.
pub fn grid_step(half_height: f64) -> (f64, f64) {
    let raw = (half_height / 10.0).max(1e-3);
    let p = 10f64.powf(raw.log10().floor());
    let minor = if raw / p < 2.0 {
        p
    } else if raw / p < 5.0 {
        2.0 * p
    } else {
        5.0 * p
    };
    (minor, minor * 5.0)
}

/// How wide the cross-fade between two grid levels is, in powers of ten of the view's height
/// (#9570): about 19% of zoom either side of the switch. The switches are at least
/// `log10(2)` apart, so two cross-fades never overlap.
const GRID_LEVEL_BLEND: f64 = 0.15;

/// One level of the drawn grid: its spacing and how strongly it is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridLevel {
    pub minor: f64,
    pub major: f64,
    /// 0..1, the level's share of the cross-fade; the levels drawn at one zoom add up to 1. The
    /// lines only this level has may be drawn weaker than its share ([`grid_lines`]).
    pub weight: f32,
}

/// Where the view is in a cross-fade between two grid levels (#9570).
#[derive(Clone, Copy, Debug)]
struct GridBlend {
    /// The finer and the coarser level's minor steps.
    lo: f64,
    hi: f64,
    /// 0..1 through the cross-fade; the switch of [`grid_step`] is at 0.5.
    s: f64,
    /// Whether the coarser step is a whole multiple of the finer one (1 to 2, 5 to 10), so that
    /// every coarser line is also a finer one. From 2 to 5 it is not.
    nested: bool,
}

impl GridBlend {
    /// The coarser level's share of the cross-fade: a smoothstep through it.
    fn share_hi(&self) -> f32 {
        smoothstep(self.s) as f32
    }

    /// How strongly the lines only one of the two levels has are drawn (finer, coarser).
    ///
    /// Nested levels cross-fade as their shares do. Levels that do not nest (2 and 5: lines at
    /// 0, 2, 4, 5, 6, 8, 10 mm) look restless drawn together, so the finer level's own lines fade
    /// out over the first half of the cross-fade and the coarser level's own lines fade in over
    /// the second: the two are never seen at once. The lines both share (every 10 mm, the finer
    /// level's major lines) stay at full strength throughout, so the grid never goes blank.
    fn own(&self) -> (f32, f32) {
        if self.nested {
            let hi = self.share_hi();
            (1.0 - hi, hi)
        } else {
            (smoothstep(1.0 - 2.0 * self.s) as f32, smoothstep(2.0 * self.s - 1.0) as f32)
        }
    }
}

fn smoothstep(x: f64) -> f64 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// The cross-fade the view of the given half height is in, if any (#9570).
fn grid_blend(half_height: f64) -> Option<GridBlend> {
    let (minor, _) = grid_step(half_height);
    let raw = half_height / 10.0;
    // grid_step's floor: no finer level is ever drawn, so there is nothing to fade to.
    if !raw.is_finite() || !minor.is_finite() || raw <= 1e-3 || minor <= 0.0 {
        return None;
    }
    let p = 10f64.powf((minor.log10() + 1e-9).floor());
    let mantissa = (minor / p).round();
    // The neighbouring steps of the 1, 2, 5 sequence.
    let (finer, coarser) = if mantissa < 1.5 {
        (minor * 0.5, minor * 2.0)
    } else if mantissa < 3.5 {
        (minor * 0.5, minor * 2.5)
    } else {
        (minor * 0.4, minor * 2.0)
    };
    let half = GRID_LEVEL_BLEND * 0.5;
    let l = raw.log10();
    // Zooming out, the switch into the coarser level comes at `log10(coarser)`; zooming in,
    // the switch into this level was at `log10(minor)`, below which the finer level returns.
    let (lo, hi) = if coarser.log10() - l < half {
        (minor, coarser)
    } else if l - minor.log10() < half && finer >= 1e-3 * (1.0 - 1e-9) {
        (finer, minor)
    } else {
        return None;
    };
    let s = ((l - hi.log10()) / GRID_LEVEL_BLEND + 0.5).clamp(0.0, 1.0);
    // The switch from 2 to 5 is the only one whose coarser step is not a whole multiple of the
    // finer one. It takes the same zoom range as the others, so the grid is cross-fading at the
    // same zooms as before; its two halves are each as long as half a nested cross-fade.
    let nested = ((hi / lo) - (hi / lo).round()).abs() < 1e-6;
    Some(GridBlend { lo, hi, s, nested })
}

/// The grid levels drawn for a view of the given half height (#9570).
///
/// [`grid_step`] switches the spacing in a step (1, 2, 5 times a power of ten), so the whole grid
/// used to swap at once as the view zoomed through a switch, popping over and over while
/// scrolling. Near a switch the drawn grid now cross-fades: the outgoing and incoming levels are
/// both drawn, weighted by how far the zoom is through the switch (from 2 to 5 one after the
/// other, see [`grid_lines`]). Away from a switch only the level [`grid_step`] gives is drawn, at
/// full strength, so the grid looks as it did. The level with the larger weight is always
/// [`grid_step`]'s, which sketch snapping, dash lengths and the tools keep using unchanged: only
/// the drawing fades.
pub fn grid_levels(half_height: f64) -> Vec<GridLevel> {
    let Some(b) = grid_blend(half_height) else {
        let (minor, major) = grid_step(half_height);
        return vec![GridLevel { minor, major, weight: 1.0 }];
    };
    let w_hi = b.share_hi();
    [GridLevel { minor: b.lo, major: b.lo * 5.0, weight: 1.0 - w_hi }, GridLevel { minor: b.hi, major: b.hi * 5.0, weight: w_hi }]
        .into_iter()
        .filter(|g| g.weight > 0.0)
        .collect()
}

/// One line of the drawn grid (#9570), across one of the grid's axes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridLineAt {
    /// Where the line crosses the axis, measured from the grid's centre.
    pub offset: f64,
    /// How strongly it is drawn (0..1), before the fade with the distance ([`GridFade`]).
    pub weight: f32,
    /// How much of that weight is a major line (0..1): the line's colour and width blend from
    /// the minor to the major ones by it.
    pub major: f32,
}

/// The lines of the grid across one axis, for a view of the given half height: every level of
/// [`grid_levels`], at the grid's own places on the plane (multiples of its step from the plane's
/// origin), so that a level fading in lies where it will stay once it is the only one. `at` is
/// the grid's centre on that axis, measured from the plane's origin; the lines reach `reach`
/// either side of it, at most `cap` steps of each level. A line two levels share is drawn once,
/// with their weights added; a line only one level has is drawn as strongly as that level's own
/// lines, which from 2 to 5 fade one after the other, never both at once.
pub fn grid_lines(half_height: f64, at: f64, reach: f64, cap: i64) -> Vec<GridLineAt> {
    if !at.is_finite() {
        return Vec::new();
    }
    // (minor step, share of the cross-fade, strength of its own lines) per level.
    let levels: Vec<(f64, f32, f32)> = match grid_blend(half_height) {
        Some(b) => {
            let (share_hi, (own_lo, own_hi)) = (b.share_hi(), b.own());
            vec![(b.lo, 1.0 - share_hi, own_lo), (b.hi, share_hi, own_hi)]
        }
        None => vec![(grid_step(half_height).0, 1.0, 1.0)],
    };
    // (line, own strength, shared by two levels)
    let mut out: Vec<(GridLineAt, f32, bool)> = Vec::new();
    for &(step, share, own) in &levels {
        let k0 = (at / step).round();
        if !k0.is_finite() {
            continue;
        }
        let n = ((reach / step) as i64).clamp(1, cap.max(1));
        for i in -n..=n {
            let k = (k0 as i64).saturating_add(i);
            let major = if k.rem_euclid(5) == 0 { 1.0 } else { 0.0 };
            out.push((GridLineAt { offset: k as f64 * step - at, weight: share, major }, own, false));
        }
    }
    out.sort_by(|a, b| a.0.offset.total_cmp(&b.0.offset));
    let tol = levels.iter().map(|l| l.0).fold(f64::INFINITY, f64::min) * 1e-6;
    let mut merged: Vec<(GridLineAt, f32, bool)> = Vec::with_capacity(out.len());
    for (line, own, _) in out {
        match merged.last_mut() {
            Some((last, _, shared)) if (line.offset - last.offset).abs() <= tol => {
                let weight = last.weight + line.weight;
                last.major = if weight > 0.0 { (last.major * last.weight + line.major * line.weight) / weight } else { last.major };
                last.weight = weight.min(1.0);
                *shared = true;
            }
            _ => merged.push((line, own, false)),
        }
    }
    merged.into_iter().map(|(line, own, shared)| if shared { line } else { GridLineAt { weight: own, ..line } }).filter(|l| l.weight > 0.0).collect()
}

/// The step of the grid's lines the viewport draws at this zoom (#9843).
///
/// Away from a cross-fade it is the step of the level the grid is drawn at. In a cross-fade that
/// level is the one with the bigger weight, and its lines are drawn as long as it has lines of
/// its own: the levels 2 and 5 do not nest, so each of them fades its own lines out over its
/// half of the cross-fade, and at the middle of it only the lines the two share (every 10 mm)
/// are drawn. The crossings the user can see are then those, and there is no crossing at 5 mm
/// to snap to.
pub fn grid_drawn_step(half_height: f64) -> f64 {
    let (minor, _) = grid_step(half_height);
    let Some(b) = grid_blend(half_height) else {
        return minor;
    };
    let (own_lo, own_hi) = b.own();
    // A nested level that still draws its own lines is drawn at its own step: the crossings the
    // user sees are the finer ones.
    if b.nested && own_lo > 0.0 {
        return b.lo;
    }
    // The level with the bigger weight is the one the grid is drawn at (`grid_step`).
    let (level, own) = if b.s >= 0.5 { (b.hi, own_hi) } else { (b.lo, own_lo) };
    if own > 0.0 { level } else { grid_shared_step(&b) }
}

/// The step of the lines two levels of a cross-fade share: the lines both of them draw.
///
/// The levels that nest (1 and 2, 5 and 10) share the lines of the coarser one. The levels that
/// do not nest (2 and 5) share only the lines every 10 mm, which is the finer level's major
/// step.
fn grid_shared_step(b: &GridBlend) -> f64 {
    if b.nested { b.hi } else { b.lo * 5.0 }
}

/// How the grid fades out with the distance from the middle of the view (#4214).
///
/// The grid used to stop at a fixed distance, so its edge showed as a cut across the picture. It
/// now fades: at full strength near the middle of the view, and gone before the grid's own edge
/// and before the picture's far plane cuts the ground, so that neither of them shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridFade {
    /// The distance from the middle of the view where the grid starts to dim.
    pub start: f64,
    /// The distance from the middle of the view where the grid is gone.
    pub end: f64,
}

impl GridFade {
    /// The fade for a view of the given half height, seen at the given slant (the cosine of the
    /// angle between the view and the grid's plane), over a grid of the given half size, that
    /// reaches no further than `far_reach` before the picture's far plane cuts the ground.
    pub fn new(half_height: f64, slant: f64, ext: f64, far_reach: f64) -> Self {
        // The distance from the middle of the view to its edge, measured on the grid's plane: the
        // ground stretches as it is seen at a slant, as the grid's own size does (#3087).
        let reach = half_height / slant.abs().max(0.25);
        // The grid is gone before its own edge, and before the far plane cuts the ground, so that
        // neither of them shows as a cut.
        let end = (2.2 * reach).min(ext).min(far_reach);
        GridFade { start: 0.6 * reach.min(end), end }
    }

    /// How much of the grid is left at a distance from the middle of the view.
    pub fn alpha(&self, d: f64) -> f32 {
        if !d.is_finite() {
            return 0.0;
        }
        if d <= self.start {
            return 1.0;
        }
        if d >= self.end || self.end <= self.start {
            return 0.0;
        }
        ((self.end - d) / (self.end - self.start)) as f32
    }

    /// The pieces the grid's line at the given offset from the middle is drawn in: each with the
    /// alpha the fade gives it at the distance where it lies. The line runs along the grid's other
    /// axis; it is drawn whole where the grid is at full strength, and in pieces that fade
    /// towards its ends.
    pub fn pieces(&self, offset: f64) -> Vec<(f64, f64, f32)> {
        let mut out = Vec::new();
        let off = offset.abs();
        if !(self.end > 0.0) || off >= self.end {
            return out;
        }
        // Where the grid is still at full strength.
        let solid = if off < self.start { (self.start * self.start - off * off).max(0.0).sqrt() } else { 0.0 };
        if solid > 0.0 {
            out.push((-solid, solid, 1.0));
        }
        // The fading ends, in pieces whose distance from the middle grows evenly, so that the
        // grid dims evenly too.
        let from = off.max(self.start);
        for k in 0..GRID_FADE_STEPS {
            let (d0, d1) =
                (from + (self.end - from) * k as f64 / GRID_FADE_STEPS as f64, from + (self.end - from) * (k + 1) as f64 / GRID_FADE_STEPS as f64);
            let (s0, s1) = ((d0 * d0 - off * off).max(0.0).sqrt(), (d1 * d1 - off * off).max(0.0).sqrt());
            let alpha = self.alpha((d0 + d1) * 0.5);
            if alpha > 0.0 {
                out.push((s0, s1, alpha));
                out.push((-s1, -s0, alpha));
            }
        }
        out
    }
}

/// How many pieces a grid line's fading end is drawn in: enough that the grid dims evenly, few
/// enough that the grid stays cheap to draw.
const GRID_FADE_STEPS: u32 = 12;

/// How far the grid reaches before the picture's far plane cuts the ground: the distance on the
/// grid's plane from the middle of the view to where the far plane ends it (#4214). Looking
/// straight down, the ground's depth does not grow and the far plane never cuts it.
///
/// `c` is the middle of the view and `n` the grid's normal, both in the world.
pub fn grid_far_reach(cam: &Camera, far: f64, c: Vec3, n: Vec3) -> f64 {
    let eye = cam.eye();
    // The direction the camera looks, and where it looks on the grid's plane: the ground's
    // forward direction, which the depth grows along.
    let view = -cam.back();
    let forward = view - n * view.dot(n);
    let len = forward.len();
    if len < 1e-9 {
        return f64::INFINITY;
    }
    let depth = (c - eye).dot(view);
    if far <= depth {
        return 0.0;
    }
    (far - depth) / len
}

/// The grid's half size for a view of the given half height, seen at the given slant (the cosine
/// of the angle between the view and the grid's plane): the grid reaches as far on its plane as
/// the view is wide, which stretches as the plane is seen at a slant (#3087).
///
/// The picture and the window share it, so that they draw the same grid (#4214).
pub fn grid_ext(half_height: f64, major: f64, slant: f64) -> f64 {
    let major = major.max(1e-3);
    let ext = (half_height * 3.0 / major).ceil() * major;
    (ext / slant.abs().max(0.25) / major).ceil() * major
}

/// One grid segment in world coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridSegment {
    pub a: Vec3,
    pub b: Vec3,
    /// The fade to draw it at (0..1); includes the line's cross-fade weight.
    pub alpha: f32,
    /// How much of the segment is the major look (0..1), for the caller's two colours.
    pub major: f32,
}

/// A grid's placement on a plane: the centre, its axes, the plane origin the line offsets are
/// measured from, and the fade.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridFrame {
    pub centre: Vec3,
    pub u: Vec3,
    pub v: Vec3,
    pub origin: Vec3,
    pub fade: GridFade,
}

/// The grid on a frame as world segments.
pub fn grid_segments_on(cam: &Camera, frame: &GridFrame, cap: i64) -> Vec<GridSegment> {
    let at_u = (frame.centre - frame.origin).dot(frame.u);
    let at_v = (frame.centre - frame.origin).dot(frame.v);
    let mut out = Vec::new();
    // Two lines through the point `frame.centre + frame.u * t`: one along v, one along u.
    for (along, across, at) in [(frame.v, frame.u, at_u), (frame.u, frame.v, at_v)] {
        for line in grid_lines(cam.half_height(), at, frame.fade.end, cap) {
            for (s0, s1, alpha) in frame.fade.pieces(line.offset) {
                out.push(GridSegment {
                    a: frame.centre + across * line.offset + along * s0,
                    b: frame.centre + across * line.offset + along * s1,
                    alpha: alpha * line.weight,
                    major: line.major,
                });
            }
        }
    }
    out
}

/// Where the ground grid is centred: where the view's axis (`back`, through the camera's target
/// `tf`) meets the plane through the origin with normal `n`, so the middle of the view has grid
/// (#3087). At a grazing slant, return smoothly toward the point under the target: the actual
/// intersection runs beyond the visible grid and jumps to the other side at the horizon.
pub fn grid_centre(tf: Vec3, back: Vec3, n: Vec3, ext: f64) -> Vec3 {
    let height = tf.dot(n);
    let under = tf - n * height;
    let slant = back.dot(n);
    // Use the same minimum slant as grid_ext and GridFade. Above it, this is the view-axis
    // intersection; below it, the offset tends continuously to zero instead of infinity.
    let reach_slant = slant.abs().max(0.25);
    let along = (back - n * slant) * (-height * slant / (reach_slant * reach_slant));
    let len = along.len();
    if len > ext && len > 0.0 { under + along * (ext / len) } else { under + along }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{StandardView, UpAxis};

    /// The minor step is 1, 2 or 5 times a power of ten near a tenth of the view's half height,
    /// the major five minor; a view with no height still has a step.
    #[test]
    fn the_grid_steps_by_one_two_and_five_with_the_view() {
        assert_eq!(grid_step(1000.0), (100.0, 500.0));
        assert_eq!(grid_step(300.0), (20.0, 100.0));
        assert_eq!(grid_step(600.0), (50.0, 250.0));
        assert_eq!(grid_step(10.0), (1.0, 5.0));
        let (minor, major) = grid_step(0.0);
        assert!(minor > 0.0 && major == minor * 5.0);
    }

    /// #4214: the grid's half size, the one the picture and the window share: the grid reaches as
    /// far on its plane as the view is wide, and further as the plane is seen at a slant.
    #[test]
    fn the_grid_reaches_further_as_its_plane_is_seen_at_a_slant() {
        let major = grid_step(40.0).1;
        let flat = grid_ext(40.0, major, 1.0);
        assert_eq!(flat, (40.0 * 3.0 / major).ceil() * major, "seen face on, the grid reaches as far as the view is wide");
        assert!(grid_ext(40.0, major, 0.5) > flat, "seen at a slant, the grid reaches further on its plane");
        // A plane seen edge-on keeps a usable size.
        assert!(grid_ext(40.0, major, 0.0).is_finite());
    }

    /// #4214: the grid's fade is at full strength in the middle of the view, and gone before the
    /// grid's own edge and before the picture's far plane cuts the ground, so that neither of them
    /// shows as a cut.
    #[test]
    fn the_grid_fades_out_before_its_edge_and_before_the_far_plane() {
        let fade = GridFade::new(40.0, 1.0, 120.0, 90.0);
        assert!(fade.start < fade.end, "the fade has a width");
        assert!(fade.end <= 90.0, "the grid is gone before the far plane cuts the ground at 90");
        assert!(fade.end <= 120.0, "the grid is gone before its own edge at 120");
        assert_eq!(fade.alpha(0.0), 1.0, "the middle of the grid is at full strength");
        assert_eq!(fade.alpha(fade.start), 1.0, "the grid is at full strength up to {}", fade.start);
        assert_eq!(fade.alpha(fade.end), 0.0, "the grid is gone at {}", fade.end);
        assert_eq!(fade.alpha(fade.end * 2.0), 0.0, "the grid does not come back");
        assert_eq!(fade.alpha(f64::NAN), 0.0, "a distance that is not a number fades the grid away");
        // The fade falls evenly.
        let mid = (fade.start + fade.end) * 0.5;
        assert!((fade.alpha(mid) - 0.5).abs() < 0.01, "the fade falls evenly: {} at {mid}", fade.alpha(mid));
    }

    /// #9570: zooming through a switch of the grid's spacing used to swap the whole grid at once, so
    /// scrolling made it pop over and over. The drawn grid now cross-fades: zoomed in small steps
    /// through a switch, no line's strength jumps from one step to the next, while away from a
    /// switch only [`grid_step`]'s level is drawn, at full strength, as before.
    #[test]
    fn the_grid_cross_fades_between_levels_as_the_view_zooms() {
        // Every 1, 2, 5 switch, zoomed by 1% a step from a half height of 8 to 1200.
        let strengths = |hh: f64| {
            let mut m = std::collections::BTreeMap::new();
            // The grid's centre off the origin, as when the view has panned: each level's lines
            // stay on the plane's own places.
            let at = 37.0;
            for l in grid_lines(hh, at, 60.0 * hh, 4000) {
                let world = l.offset + at;
                // Weight and major share, keyed by the line's place on the plane (to 1e-6 mm).
                m.insert((world * 1e6).round() as i64, (l.weight, l.major));
            }
            m
        };
        let mut hh = 8.0;
        let mut prev = strengths(hh);
        let mut faded = 0;
        while hh < 1200.0 {
            let next_hh = hh * 1.01;
            let next = strengths(next_hh);
            // Only lines both steps draw: compare within the smaller reach, short of its ends, where
            // each level stops at a whole number of its steps.
            let reach = ((60.0 * hh - hh / 4.0) * 1e6) as i64;
            for key in prev.keys().chain(next.keys()).filter(|k| (**k - 37_000_000).abs() < reach) {
                let (a, ma) = prev.get(key).copied().unwrap_or((0.0, 0.0));
                let (b, mb) = next.get(key).copied().unwrap_or((0.0, 0.0));
                assert!((a - b).abs() < 0.1, "the line at {} mm jumps from {a} to {b} zooming from {hh} to {next_hh}", *key as f64 * 1e-6);
                if a > 0.05 && b > 0.05 {
                    assert!(
                        (ma - mb).abs() < 0.1,
                        "the line at {} mm turns from major {ma} to {mb} zooming from {hh} to {next_hh}",
                        *key as f64 * 1e-6
                    );
                }
            }
            let levels = grid_levels(next_hh);
            let total: f32 = levels.iter().map(|g| g.weight).sum();
            assert!((total - 1.0).abs() < 1e-5, "the levels at {next_hh} add up to {total}");
            // The strongest level is the one snapping and the tools use.
            let strongest = levels.iter().fold(levels[0], |a, g| if g.weight > a.weight { *g } else { a });
            assert_eq!((strongest.minor, strongest.major), grid_step(next_hh), "at {next_hh} the drawn grid's main level is grid_step's");
            faded += usize::from(levels.len() == 2);
            prev = next;
            hh = next_hh;
        }
        assert!(faded > 0, "some steps cross-fade");
        // Away from a switch (raw step 3 lies between the 2 and 5 switches), one level at full
        // strength, as the grid always was.
        assert_eq!(grid_levels(30.0), vec![GridLevel { minor: 2.0, major: 10.0, weight: 1.0 }]);
        // grid_step's floor and hostile heights draw one level.
        for hh in [0.0, 1e-9, -5.0, f64::NAN, f64::INFINITY] {
            assert_eq!(grid_levels(hh).len(), 1, "{hh}");
            let _ = grid_lines(hh, f64::NAN, 10.0, 10);
            let _ = grid_lines(hh, 1e300, 1e300, 10);
        }
    }

    /// #9570: the levels 2 and 5 do not nest, and drawn together they gave lines at 0, 2, 4, 5, 6,
    /// 8, 10 mm, which looked restless. Zooming through that switch the finer level's own lines are
    /// gone before the coarser level's own lines appear, while the lines both share (every 10 mm)
    /// stay at full strength, so the grid never goes blank. Checked in every decade drawn.
    #[test]
    fn the_grid_never_draws_two_levels_that_do_not_nest_at_once() {
        for scale in [0.1, 1.0, 10.0, 100.0] {
            // grid_step switches from 2 to 5 (times `scale`) at a half height of 50 times `scale`.
            let (mut hh, end) = (30.0 * scale, 80.0 * scale);
            let (mut saw_fine, mut saw_coarse) = (false, false);
            while hh < end {
                let lines = grid_lines(hh, 0.0, 60.0 * scale, 4000);
                let at = |x: f64| lines.iter().filter(|l| (l.offset - x * scale).abs() < 1e-6 * scale).map(|l| l.weight).sum::<f32>();
                // 2, 4, 6, 8: only the finer level; 5, 15: only the coarser.
                let fine = [2.0, 4.0, 6.0, 8.0].iter().map(|x| at(*x)).fold(0.0f32, f32::max);
                let coarse = [5.0, 15.0].iter().map(|x| at(*x)).fold(0.0f32, f32::max);
                assert!(fine == 0.0 || coarse == 0.0, "at {hh} the 2 and 5 levels are both drawn ({fine}, {coarse})");
                saw_fine |= fine > 0.0 && fine < 1.0;
                saw_coarse |= coarse > 0.0 && coarse < 1.0;
                for x in [0.0, 10.0, 20.0] {
                    assert!(at(x) > 0.999, "at {hh} the line at {x} mm both levels share is drawn at {}", at(x));
                }
                hh *= 1.005;
            }
            assert!(saw_fine && saw_coarse, "both levels fade rather than swap (scale {scale})");
        }
        // The nested switches still cross-fade both levels together: from 1 to 2 (half height 20).
        let lines = grid_lines(20.0, 0.0, 20.0, 4000);
        let at = |x: f64| lines.iter().filter(|l| (l.offset - x).abs() < 1e-6).map(|l| l.weight).sum::<f32>();
        assert!(at(1.0) > 0.3 && at(1.0) < 0.7 && at(2.0) > 0.999, "{} {}", at(1.0), at(2.0));
    }

    /// #1470, #1993: the grid lies on the ground unless seen edge-on, switches walls only near
    /// side views, has hysteresis, and handles hostile view directions safely.
    #[test]
    fn grid_plane_follows_side_views_and_keeps_hysteresis_and_hostile_safety() {
        let back_at = |yaw_deg: f64, pitch_deg: f64| {
            let cam = Camera { yaw: yaw_deg.to_radians(), pitch: pitch_deg.to_radians(), ..Camera::default() };
            cam.to_frame(cam.back())
        };
        for up in [UpAxis::Z, UpAxis::Y] {
            for view in [StandardView::Top, StandardView::Bottom, StandardView::Iso] {
                let (yaw, pitch) = view.angles();
                let cam = Camera { yaw, pitch, up, ..Camera::default() };
                let (u, v) = grid_plane(cam.to_frame(cam.back()), GridPlane::default()).axes();
                let n = cam.from_frame(u).cross(cam.from_frame(v));
                assert_eq!(n.dot(up.dir()).abs(), 1.0, "{view:?} {up:?}");
            }
            for view in [StandardView::Front, StandardView::Back, StandardView::Right, StandardView::Left] {
                let (yaw, pitch) = view.angles();
                let cam = Camera { yaw, pitch, up, ..Camera::default() };
                let (u, v) = grid_plane(cam.to_frame(cam.back()), GridPlane::default()).axes();
                let n = cam.from_frame(u).cross(cam.from_frame(v));
                assert!(n.dot(up.dir()).abs() < 1e-9, "{view:?} {up:?}");
                assert!((n.dot(cam.back()).abs() - 1.0).abs() < 1e-9, "{view:?} {up:?}");
            }
        }
        assert_eq!(grid_plane(back_at(0.0, 9.0), GridPlane::Ground), GridPlane::Xz);
        assert_eq!(grid_plane(back_at(0.0, 12.0), GridPlane::Ground), GridPlane::Ground);
        assert_eq!(grid_plane(back_at(0.0, 12.0), GridPlane::Xz), GridPlane::Xz);
        assert_eq!(grid_plane(back_at(0.0, 16.0), GridPlane::Xz), GridPlane::Ground);
        assert_eq!(grid_plane(back_at(30.0, -12.0), GridPlane::Yz), GridPlane::Yz);
        assert_eq!(grid_plane(back_at(44.0, 3.0), GridPlane::Xz), GridPlane::Xz);
        assert_eq!(grid_plane(back_at(50.0, 3.0), GridPlane::Xz), GridPlane::Xz);
        assert_eq!(grid_plane(back_at(80.0, 3.0), GridPlane::Xz), GridPlane::Yz);
        for back in [Vec3::new(f64::NAN, 0.0, 0.0), Vec3::new(0.0, f64::NEG_INFINITY, 0.0), Vec3::ZERO] {
            assert_eq!(grid_plane(back, GridPlane::Xz), GridPlane::Ground, "{back:?}");
        }
    }

    /// #3087: `grid_centre` tracks the view-axis intersection on the plane, stays within `ext` at
    /// grazing angles, and moves continuously across the horizon.
    #[test]
    fn grid_centre_stays_within_reach_and_is_continuous_across_the_horizon() {
        let n = Vec3::Z;
        let tf = Vec3::new(0.0, 0.0, 10.0);
        let back = Vec3::new(0.0, -1.0, 1.0).normalized().unwrap();
        let c = grid_centre(tf, back, n, 100.0);
        assert!((c - Vec3::new(0.0, 10.0, 0.0)).len() < 1e-9, "{c:?}");

        let back = Vec3::new(0.0, -1.0, 0.01).normalized().unwrap();
        let c = grid_centre(tf, back, n, 120.0);
        assert!(c.z.abs() < 1e-9 && c.len() <= 120.0 + 1e-9 && c.y > 0.0, "{c:?}");

        let target = Vec3::X * 2.0 + n * 13.0;
        for step in -20..20 {
            let p0 = f64::from(step) * 0.0001;
            let p1 = f64::from(step + 1) * 0.0001;
            let a = grid_centre(target, Vec3::X * p0.cos() + n * p0.sin(), n, 120.0);
            let b = grid_centre(target, Vec3::X * p1.cos() + n * p1.sin(), n, 120.0);
            assert!(a.dot(n).abs() < 1e-9 && b.dot(n).abs() < 1e-9);
            assert!(a.dist(b) < 0.1, "{a:?} -> {b:?}");
        }
    }

    #[test]
    fn grid_far_reach_handles_vertical_slanted_and_past_far_views() {
        let top = Camera { pitch: std::f64::consts::FRAC_PI_2, ..Camera::default() };
        assert_eq!(grid_far_reach(&top, 1000.0, Vec3::ZERO, Vec3::Z), f64::INFINITY);

        let slanted = Camera { yaw: 0.0, pitch: 0.25, distance: 100.0, ..Camera::default() };
        assert_eq!(grid_far_reach(&slanted, 50.0, Vec3::ZERO, Vec3::Z), 0.0);
        let reach = grid_far_reach(&slanted, 500.0, Vec3::ZERO, Vec3::Z);
        assert!(reach > 300.0 && reach.is_finite(), "slanted reach: {reach}");
    }

    /// #9843: `grid_drawn_step` matches the lines `grid_lines` actually draws.
    #[test]
    fn grid_drawn_step_follows_visible_crossings_across_blends() {
        assert_eq!(grid_drawn_step(30.0), 2.0);
        assert_eq!(grid_drawn_step(50.0), 10.0);
        assert_eq!(grid_drawn_step(110.0), 5.0);
    }

    /// [`grid_segments_on`] is exactly the grid's lines drawn in pieces at the fade's strength.
    #[test]
    fn grid_segments_on_matches_grid_lines_and_the_fade() {
        let cam = Camera::default();
        let fade = GridFade::new(cam.half_height(), 0.8, 100.0, 70.0);
        let frame = GridFrame { centre: Vec3::new(3.0, -4.0, 0.0), u: Vec3::X, v: Vec3::Y, origin: Vec3::new(1.0, 2.0, 0.0), fade };
        let got = grid_segments_on(&cam, &frame, 7);
        let mut want = Vec::new();
        let (at_u, at_v) = ((frame.centre - frame.origin).dot(frame.u), (frame.centre - frame.origin).dot(frame.v));
        for (along, across, at) in [(frame.v, frame.u, at_u), (frame.u, frame.v, at_v)] {
            for line in grid_lines(cam.half_height(), at, frame.fade.end, 7) {
                for (s0, s1, alpha) in frame.fade.pieces(line.offset) {
                    want.push(GridSegment {
                        a: frame.centre + across * line.offset + along * s0,
                        b: frame.centre + across * line.offset + along * s1,
                        alpha: alpha * line.weight,
                        major: line.major,
                    });
                }
            }
        }
        assert!(!want.is_empty(), "the frame draws a grid");
        assert_eq!(got, want);
    }

    #[test]
    fn grid_segments_on_respects_upright_frame_and_empty_fade() {
        let mut cam = Camera { yaw: 0.0, pitch: 90.0_f64.to_radians(), distance: 50.0 / 0.4, ..Camera::default() };
        let (u, v) = GridPlane::Xz.axes();
        let fade = GridFade::new(cam.half_height(), 1.0, 120.0, 1e6);
        let frame = GridFrame { centre: Vec3::ZERO, u, v, origin: Vec3::ZERO, fade };
        let segs = grid_segments_on(&cam, &frame, 300);
        assert!(!segs.is_empty());
        assert!(segs.iter().all(|s| s.a.y.abs() < 1e-9 && s.b.y.abs() < 1e-9));
        assert!(segs.iter().any(|s| s.major > 0.0 && s.major < 1.0));
        assert!(segs.iter().any(|s| (s.a.x - s.b.x).abs() < 1e-9));
        assert!(segs.iter().any(|s| (s.a.z - s.b.z).abs() < 1e-9));
        for seg in &segs {
            assert!((0.0..=1.0).contains(&seg.alpha));
            assert!((0.0..=1.0).contains(&seg.major));
        }

        cam.up = UpAxis::Y;
        let empty = GridFrame { centre: Vec3::ZERO, u, v, origin: Vec3::ZERO, fade: GridFade { start: 0.0, end: 0.0 } };
        assert!(grid_segments_on(&cam, &empty, 300).is_empty());
    }
}
