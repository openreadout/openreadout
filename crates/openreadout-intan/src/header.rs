//! RHD2000 / RHS2000 header ("Standard Intan Header") and the data-block layout.

use openreadout_core::bytes::{Block, utf16le};

/// Magic number of `.rhd` files.
pub const RHD_MAGIC: u32 = 0xC691_2702;
/// Magic number of `.rhs` files.
pub const RHS_MAGIC: u32 = 0xD691_27AC;

/// Which chip family wrote the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    /// RHD2000 recording system (`.rhd`).
    Rhd,
    /// RHS2000 stimulation/recording system (`.rhs`).
    Rhs,
}

/// What a stored signal is (the header's signal-type code, per family).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SignalKind {
    Amplifier,
    DcAmplifier,
    Stimulation,
    Auxiliary,
    Supply,
    Temperature,
    BoardAdc,
    BoardDac,
    DigitalIn,
    DigitalOut,
}

impl SignalKind {
    /// Our trace name.
    pub fn name(self) -> &'static str {
        match self {
            SignalKind::Amplifier => "amplifier",
            SignalKind::DcAmplifier => "dc_amplifier",
            SignalKind::Stimulation => "stimulation",
            SignalKind::Auxiliary => "auxiliary",
            SignalKind::Supply => "supply",
            SignalKind::Temperature => "temperature",
            SignalKind::BoardAdc => "board_adc",
            SignalKind::BoardDac => "board_dac",
            SignalKind::DigitalIn => "digital_in",
            SignalKind::DigitalOut => "digital_out",
        }
    }
    /// From the channel record's signal-type code.
    pub fn from_code(family: Family, code: i16) -> Option<Self> {
        match (family, code) {
            (_, 0) => Some(SignalKind::Amplifier),
            (Family::Rhd, 1) => Some(SignalKind::Auxiliary),
            (Family::Rhd, 2) => Some(SignalKind::Supply),
            (_, 3) => Some(SignalKind::BoardAdc),
            (Family::Rhd, 4) | (Family::Rhs, 5) => Some(SignalKind::DigitalIn),
            (Family::Rhd, 5) | (Family::Rhs, 6) => Some(SignalKind::DigitalOut),
            (Family::Rhs, 4) => Some(SignalKind::BoardDac),
            _ => None,
        }
    }
}

/// One channel record.
#[derive(Debug, Clone, PartialEq)]
pub struct IntanChannel {
    pub native_name: String,
    pub custom_name: String,
    pub native_order: i16,
    pub custom_order: i16,
    pub signal_code: i16,
    pub enabled: bool,
    pub chip_channel: i16,
    pub command_stream: Option<i16>,
    pub board_stream: i16,
    pub impedance_ohm: f32,
    pub impedance_phase_deg: f32,
    pub group: String,
}

/// The parsed header.
#[derive(Debug, Clone)]
pub struct IntanHeader {
    pub family: Family,
    pub version: (i16, i16),
    pub sample_rate_hz: f32,
    pub dsp_enabled: bool,
    pub dsp_cutoff_hz: f32,
    pub lower_bandwidth_hz: f32,
    pub lower_settle_bandwidth_hz: Option<f32>,
    pub upper_bandwidth_hz: f32,
    pub notch_mode: i16,
    pub impedance_test_hz: f32,
    pub amp_settle_mode: Option<i16>,
    pub charge_recovery_mode: Option<i16>,
    /// Amperes per stimulation step (RHS).
    pub stim_step_a: Option<f32>,
    pub charge_recovery_limit_a: Option<f32>,
    pub charge_recovery_target_v: Option<f32>,
    pub notes: Vec<String>,
    pub temperature_sensors: i16,
    pub board_mode: i16,
    pub dc_saved: bool,
    pub reference: Option<String>,
    /// Every channel record of every enabled group (enabled or not).
    pub channels: Vec<IntanChannel>,
    pub header_len: u64,
}

impl IntanHeader {
    /// Samples per data block: 128 for RHS and RHD 2.0+, 60 for older RHD files.
    pub fn block_samples(&self) -> u64 {
        match self.family {
            Family::Rhs => 128,
            Family::Rhd if self.version.0 >= 2 => 128,
            Family::Rhd => 60,
        }
    }
    /// Enabled channels of one kind, in header order.
    pub fn enabled(&self, kind: SignalKind) -> Vec<&IntanChannel> {
        self.channels
            .iter()
            .filter(|c| {
                c.enabled && SignalKind::from_code(self.family, c.signal_code) == Some(kind)
            })
            .collect()
    }
    /// Time indices are signed from RHD 1.2 and in every RHS file.
    pub fn signed_timestamps(&self) -> bool {
        self.family == Family::Rhs || self.version >= (1, 2)
    }
}

/// A cursor over the header bytes.
#[derive(Debug)]
pub struct Cursor<'a> {
    pub block: &'a Block,
    pub at: u64,
}

impl Cursor<'_> {
    fn i16(&mut self) -> Result<i16, String> {
        let v = self.block.i16_at(self.at).ok_or("header ends early")?;
        self.at += 2;
        Ok(v)
    }
    fn f32(&mut self) -> Result<f32, String> {
        let v = self.block.f32_at(self.at).ok_or("header ends early")?;
        self.at += 4;
        Ok(v)
    }
    /// Qt `QString`: u32 byte length (0xFFFFFFFF = null) then UTF-16LE.
    fn qstring(&mut self) -> Result<String, String> {
        let n = self.block.u32_at(self.at).ok_or("header ends early")?;
        self.at += 4;
        if n == 0xFFFF_FFFF {
            return Ok(String::new());
        }
        if n % 2 != 0 || n > 1 << 20 {
            return Err(format!("string length {n} at byte {}", self.at - 4));
        }
        let b = self
            .block
            .slice(self.at, n as usize)
            .ok_or("header ends inside a string")?;
        self.at += u64::from(n);
        Ok(utf16le(b))
    }
}

/// Parse the header at the start of `block` (which must start at file offset 0).
pub fn parse_header(block: &Block) -> Result<IntanHeader, String> {
    let magic = block.u32_at(0).ok_or("file shorter than 4 bytes")?;
    let family = match magic {
        RHD_MAGIC => Family::Rhd,
        RHS_MAGIC => Family::Rhs,
        m => return Err(format!("magic number {m:#010x} is neither RHD nor RHS")),
    };
    let rhs = family == Family::Rhs;
    let mut c = Cursor { block, at: 4 };
    let version = (c.i16()?, c.i16()?);
    let sample_rate_hz = c.f32()?;
    let dsp_enabled = c.i16()? != 0;
    let dsp_cutoff_hz = c.f32()?;
    let lower_bandwidth_hz = c.f32()?;
    let lower_settle_bandwidth_hz = if rhs { Some(c.f32()?) } else { None };
    let upper_bandwidth_hz = c.f32()?;
    // desired values (not reported)
    c.f32()?;
    c.f32()?;
    if rhs {
        c.f32()?;
    }
    c.f32()?;
    let notch_mode = c.i16()?;
    c.f32()?; // desired impedance-test frequency
    let impedance_test_hz = c.f32()?;
    let (mut amp_settle_mode, mut charge_recovery_mode) = (None, None);
    let (mut stim_step_a, mut limit, mut target) = (None, None, None);
    if rhs {
        amp_settle_mode = Some(c.i16()?);
        charge_recovery_mode = Some(c.i16()?);
        stim_step_a = Some(c.f32()?);
        limit = Some(c.f32()?);
        target = Some(c.f32()?);
    }
    let notes = vec![c.qstring()?, c.qstring()?, c.qstring()?];
    let (mut temperature_sensors, mut board_mode, mut dc_saved, mut reference) =
        (0, 0, false, None);
    if rhs {
        dc_saved = c.i16()? != 0;
        board_mode = c.i16()?;
        reference = Some(c.qstring()?);
    } else {
        if version >= (1, 1) {
            temperature_sensors = c.i16()?;
        }
        if version >= (1, 3) {
            board_mode = c.i16()?;
        }
        if version.0 >= 2 {
            reference = Some(c.qstring()?);
        }
    }
    let groups = c.i16()?;
    if !(0..=1024).contains(&groups) {
        return Err(format!("{groups} signal groups"));
    }
    let mut channels = Vec::new();
    for _ in 0..groups {
        let name = c.qstring()?;
        let _prefix = c.qstring()?;
        let enabled = c.i16()? != 0;
        let n = c.i16()?;
        let _n_amp = c.i16()?;
        if !(0..=4096).contains(&n) {
            return Err(format!("signal group {name:?} has {n} channels"));
        }
        if !enabled || n == 0 {
            continue;
        }
        for _ in 0..n {
            let native_name = c.qstring()?;
            let custom_name = c.qstring()?;
            let native_order = c.i16()?;
            let custom_order = c.i16()?;
            let signal_code = c.i16()?;
            let ch_enabled = c.i16()? != 0;
            let chip_channel = c.i16()?;
            let command_stream = if rhs { Some(c.i16()?) } else { None };
            let board_stream = c.i16()?;
            for _ in 0..4 {
                c.i16()?; // Spike Scope trigger settings
            }
            let impedance_ohm = c.f32()?;
            let impedance_phase_deg = c.f32()?;
            channels.push(IntanChannel {
                native_name,
                custom_name,
                native_order,
                custom_order,
                signal_code,
                enabled: ch_enabled,
                chip_channel,
                command_stream,
                board_stream,
                impedance_ohm,
                impedance_phase_deg,
                group: name.clone(),
            });
        }
    }
    Ok(IntanHeader {
        family,
        version,
        sample_rate_hz,
        dsp_enabled,
        dsp_cutoff_hz,
        lower_bandwidth_hz,
        lower_settle_bandwidth_hz,
        upper_bandwidth_hz,
        notch_mode,
        impedance_test_hz,
        amp_settle_mode,
        charge_recovery_mode,
        stim_step_a,
        charge_recovery_limit_a: limit,
        charge_recovery_target_v: target,
        notes,
        temperature_sensors,
        board_mode,
        dc_saved,
        reference,
        channels,
        header_len: c.at,
    })
}

/// Where one kind of signal sits inside a data block.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockPart {
    pub kind: SignalKind,
    /// Byte offset of the part within a block.
    pub offset: u64,
    /// Stored channels (1 for the digital words).
    pub channels: u64,
    /// Samples per channel per block.
    pub samples: u64,
}

/// The block layout: its parts in file order and the block length.
pub fn block_layout(h: &IntanHeader) -> (Vec<BlockPart>, u64) {
    let n = h.block_samples();
    let count = |k| h.enabled(k).len() as u64;
    let mut parts = Vec::new();
    let mut at = 4 * n; // time indices
    let mut push = |kind, channels: u64, samples: u64, at: &mut u64| {
        if channels > 0 {
            parts.push(BlockPart {
                kind,
                offset: *at,
                channels,
                samples,
            });
            *at += 2 * channels * samples;
        }
    };
    let amp = count(SignalKind::Amplifier);
    match h.family {
        Family::Rhd => {
            push(SignalKind::Amplifier, amp, n, &mut at);
            push(
                SignalKind::Auxiliary,
                count(SignalKind::Auxiliary),
                n / 4,
                &mut at,
            );
            push(SignalKind::Supply, count(SignalKind::Supply), 1, &mut at);
            push(
                SignalKind::Temperature,
                u64::try_from(h.temperature_sensors.max(0)).unwrap_or(0),
                1,
                &mut at,
            );
            push(
                SignalKind::BoardAdc,
                count(SignalKind::BoardAdc),
                n,
                &mut at,
            );
            push(
                SignalKind::DigitalIn,
                u64::from(count(SignalKind::DigitalIn) > 0),
                n,
                &mut at,
            );
            push(
                SignalKind::DigitalOut,
                u64::from(count(SignalKind::DigitalOut) > 0),
                n,
                &mut at,
            );
        }
        Family::Rhs => {
            push(SignalKind::Amplifier, amp, n, &mut at);
            if h.dc_saved {
                push(SignalKind::DcAmplifier, amp, n, &mut at);
            }
            push(SignalKind::Stimulation, amp, n, &mut at);
            push(
                SignalKind::BoardAdc,
                count(SignalKind::BoardAdc),
                n,
                &mut at,
            );
            push(
                SignalKind::BoardDac,
                count(SignalKind::BoardDac),
                n,
                &mut at,
            );
            push(
                SignalKind::DigitalIn,
                u64::from(count(SignalKind::DigitalIn) > 0),
                n,
                &mut at,
            );
            push(
                SignalKind::DigitalOut,
                u64::from(count(SignalKind::DigitalOut) > 0),
                n,
                &mut at,
            );
        }
    }
    (parts, at)
}

/// `(scale, offset, unit)` for a stored sample of `kind`: value = raw × scale + offset.
pub fn scaling(h: &IntanHeader, kind: SignalKind) -> (f64, f64, Option<&'static str>) {
    match kind {
        SignalKind::Amplifier => (0.195, -6389.76, Some("µV")),
        SignalKind::DcAmplifier => (19.23, -9845.76, Some("mV")),
        SignalKind::Stimulation => (f64::from(h.stim_step_a.unwrap_or(1.0)), 0.0, Some("A")),
        SignalKind::Auxiliary => (0.000_037_4, 0.0, Some("V")),
        SignalKind::Supply => (0.000_074_8, 0.0, Some("V")),
        SignalKind::Temperature => (0.01, 0.0, Some("°C")),
        SignalKind::BoardAdc | SignalKind::BoardDac => match (h.family, h.board_mode) {
            (Family::Rhs, _) | (Family::Rhd, 13) => (0.000_312_5, -10.24, Some("V")),
            (Family::Rhd, 0) => (0.000_050_354, 0.0, Some("V")),
            (Family::Rhd, 1) => (0.000_152_59, -32768.0 * 0.000_152_59, Some("V")),
            _ => (1.0, 0.0, None),
        },
        SignalKind::DigitalIn | SignalKind::DigitalOut => (1.0, 0.0, None),
    }
}

/// Stimulation word → signed step count: bits 0–7 magnitude, bit 8 sign.
pub fn stim_steps(word: u16) -> f64 {
    // integer negation keeps a zero magnitude +0.0 even when the sign bit is set
    let mag = i32::from(word & 0xFF);
    f64::from(if word & 0x100 != 0 { -mag } else { mag })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::float_cmp)]
    fn stim_decoding() {
        assert_eq!(stim_steps(0x0005), 5.0);
        assert_eq!(stim_steps(0x0105), -5.0);
        assert_eq!(stim_steps(0x8000 | 0x0103), -3.0);
        assert!(stim_steps(0x0100).is_sign_positive());
    }

    #[test]
    fn kinds() {
        assert_eq!(
            SignalKind::from_code(Family::Rhs, 4),
            Some(SignalKind::BoardDac)
        );
        assert_eq!(
            SignalKind::from_code(Family::Rhd, 4),
            Some(SignalKind::DigitalIn)
        );
        assert_eq!(SignalKind::from_code(Family::Rhd, 9), None);
    }
}
