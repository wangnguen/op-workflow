//! The tray icon (PLAN.md §7 "Tray icon dạng gauge"): two rings of what is left in the 5-hour
//! windows (outer Claude, inner Codex) and a tooltip with all four windows. Drawn here as raw
//! RGBA so the shell only hands the pixels to Windows (core never depends on Tauri).

use opw_domain::view::{LinkStatus, Provider, ProviderLink, QuotaSnapshot, QuotaWindowKind};

/// Pixels per side; Windows scales it to the tray size (16-24 px).
pub const ICON_SIZE: u32 = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayView {
    pub tooltip: String,
    /// `ICON_SIZE`² RGBA pixels.
    pub rgba: Vec<u8>,
    /// Changes when the picture or the text would change (skip redundant updates).
    pub key: String,
}

type Rgb = (u8, u8, u8);

const CLAUDE: Rgb = (0xe0, 0x86, 0x4e);
const CODEX: Rgb = (0x8c, 0x7c, 0xf0);
const DANGER: Rgb = (0xe5, 0x48, 0x4d);
const TRACK: Rgb = (0x8a, 0x8f, 0x98);

fn left(quota: &[QuotaSnapshot], provider: Provider, kind: QuotaWindowKind) -> Option<f64> {
    quota
        .iter()
        .find(|q| q.provider == provider)?
        .windows
        .iter()
        .find(|w| w.kind == kind)
        .map(opw_quota::remaining_pct_now)
}

fn broken(links: &[ProviderLink], provider: Provider) -> bool {
    links.iter().any(|l| {
        l.provider == provider
            && matches!(
                l.status,
                LinkStatus::Error | LinkStatus::Missing | LinkStatus::SignedOut
            )
    })
}

fn pct(v: Option<f64>) -> String {
    v.map_or_else(|| "?".to_owned(), |p| format!("{p:.0}%"))
}

/// Icon and tooltip for the current quota.
pub fn tray_view(quota: &[QuotaSnapshot], links: &[ProviderLink]) -> TrayView {
    let line = |name: &str, p: Provider| {
        if broken(links, p) {
            return format!("{name}: cần kết nối lại");
        }
        match (
            left(quota, p, QuotaWindowKind::FiveHour),
            left(quota, p, QuotaWindowKind::SevenDay),
        ) {
            (None, None) => format!("{name}: chưa có số"),
            (h5, d7) => format!("{name}: 5 giờ {} · tuần {}", pct(h5), pct(d7)),
        }
    };
    let tooltip = format!(
        "op-workflow · hạn mức còn lại\n{}\n{}",
        line("Claude", Provider::Claude),
        line("Codex", Provider::Codex)
    );
    let ring = |p: Provider| {
        if broken(links, p) {
            None
        } else {
            left(quota, p, QuotaWindowKind::FiveHour)
        }
    };
    let (outer, inner) = (ring(Provider::Claude), ring(Provider::Codex));
    let rgba = draw(outer, inner);
    let key = format!(
        "{}|{}|{tooltip}",
        outer.map_or(-1, |v| v.round() as i64),
        inner.map_or(-1, |v| v.round() as i64)
    );
    TrayView { tooltip, rgba, key }
}

/// Two rings, filled clockwise from 12 o'clock by the share left; red under 10%.
pub fn draw(outer: Option<f64>, inner: Option<f64>) -> Vec<u8> {
    const SUB: u32 = 4;
    let size = ICON_SIZE as f64;
    let c = size / 2.0;
    // (radius, half width, share left, colour)
    let rings = [(12.6, 2.6, outer, CLAUDE), (6.4, 2.6, inner, CODEX)];
    let mut out = vec![0u8; (ICON_SIZE * ICON_SIZE * 4) as usize];
    for y in 0..ICON_SIZE {
        for x in 0..ICON_SIZE {
            // average of SUB×SUB samples, premultiplied
            let (mut r, mut g, mut b, mut a) = (0.0, 0.0, 0.0, 0.0);
            for sy in 0..SUB {
                for sx in 0..SUB {
                    let px = f64::from(x) + (f64::from(sx) + 0.5) / f64::from(SUB) - c;
                    let py = f64::from(y) + (f64::from(sy) + 0.5) / f64::from(SUB) - c;
                    let dist = px.hypot(py);
                    // 0 at 12 o'clock, growing clockwise, 0..1
                    let turn = (px.atan2(-py) / std::f64::consts::TAU).rem_euclid(1.0);
                    for (radius, half, share, colour) in rings {
                        if (dist - radius).abs() > half {
                            continue;
                        }
                        let (rgb, alpha) = match share {
                            Some(s) if turn <= s / 100.0 => (if s < 10.0 { DANGER } else { colour }, 1.0),
                            _ => (TRACK, 0.45),
                        };
                        r += f64::from(rgb.0) * alpha;
                        g += f64::from(rgb.1) * alpha;
                        b += f64::from(rgb.2) * alpha;
                        a += alpha;
                        break;
                    }
                }
            }
            let n = f64::from(SUB * SUB);
            let i = ((y * ICON_SIZE + x) * 4) as usize;
            if a > 0.0 {
                out[i] = (r / a).round() as u8;
                out[i + 1] = (g / a).round() as u8;
                out[i + 2] = (b / a).round() as u8;
                out[i + 3] = (a / n * 255.0).round() as u8;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use opw_domain::view::QuotaWindow;

    fn snap(provider: Provider, used5: f64) -> QuotaSnapshot {
        QuotaSnapshot {
            provider,
            windows: vec![
                QuotaWindow {
                    kind: QuotaWindowKind::FiveHour,
                    used_pct: used5,
                    resets_at: None,
                },
                QuotaWindow {
                    kind: QuotaWindowKind::SevenDay,
                    used_pct: 88.0,
                    resets_at: None,
                },
            ],
            source: "x".into(),
            observed_at: "x".into(),
            plan: None,
        }
    }

    fn pixel(rgba: &[u8], x: u32, y: u32) -> [u8; 4] {
        let i = ((y * ICON_SIZE + x) * 4) as usize;
        [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
    }

    #[test]
    fn tooltip_lists_what_is_left() {
        let v = tray_view(&[snap(Provider::Claude, 38.0)], &[]);
        assert_eq!(
            v.tooltip,
            "op-workflow · hạn mức còn lại\nClaude: 5 giờ 62% · tuần 12%\nCodex: chưa có số"
        );
        // Windows cuts tray tooltips at 127 characters
        assert!(v.tooltip.chars().count() < 128);
        assert_eq!(v.rgba.len(), (ICON_SIZE * ICON_SIZE * 4) as usize);
        let same = tray_view(&[snap(Provider::Claude, 38.0)], &[]);
        assert_eq!(v.key, same.key);
        assert_ne!(v.key, tray_view(&[snap(Provider::Claude, 60.0)], &[]).key);
    }

    #[test]
    fn rings_fill_clockwise_from_the_top() {
        // Claude half left: top-right of the outer ring filled, top-left only the track
        let rgba = draw(Some(50.0), None);
        let right = pixel(&rgba, 16 + 12, 16);
        let left = pixel(&rgba, 16 - 13, 16);
        assert_eq!(&right[..3], &[CLAUDE.0, CLAUDE.1, CLAUDE.2]);
        assert!(right[3] > 200);
        assert_eq!(&left[..3], &[TRACK.0, TRACK.1, TRACK.2]);
        assert!(left[3] < 160);
        // centre and corners stay transparent
        assert_eq!(pixel(&rgba, 16, 16)[3], 0);
        assert_eq!(pixel(&rgba, 0, 0)[3], 0);
        // nearly empty turns red
        let low = draw(None, Some(5.0));
        let top = pixel(&low, 16, 16 - 7);
        assert_eq!(&top[..3], &[DANGER.0, DANGER.1, DANGER.2]);
    }
}
