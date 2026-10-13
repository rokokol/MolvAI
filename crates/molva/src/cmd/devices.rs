// SPDX-License-Identifier: MIT
//! `molva devices [--json]` — which microphones the system sees

use molva_core::domain::audio::DeviceInfo;
use molva_core::infra::audio::list_input_devices;

/// Print the input devices: a table for a person, JSON for scripts (Y-15 — everything goes to stdout)
pub(crate) fn run(json: bool) -> anyhow::Result<()> {
    let devices = list_input_devices()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&devices)?);
    } else {
        print!("{}", render_table(&devices));
    }
    Ok(())
}

/// The table "name / default / rates"
fn render_table(devices: &[DeviceInfo]) -> String {
    let name_width = devices
        .iter()
        .map(|d| d.name.chars().count())
        .max()
        .unwrap_or(0)
        .max("Device".chars().count());

    let mut out = format!(
        "{:<name_width$}  {:^12}  {}\n",
        "Device",
        "Default",
        "Rates, Hz",
        name_width = name_width
    );
    for device in devices {
        // Spaces instead of a width by characters: device names can contain non-ASCII letters
        let padding = " ".repeat(name_width.saturating_sub(device.name.chars().count()));
        let rates = if device.sample_rates.is_empty() {
            "—".to_string()
        } else {
            device
                .sample_rates
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        };
        out.push_str(&format!(
            "{}{}  {:^12}  {}\n",
            device.name,
            padding,
            if device.is_default { "yes" } else { "" },
            rates
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn devices() -> Vec<DeviceInfo> {
        vec![
            DeviceInfo {
                name: "Микрофон (встроенный)".into(),
                is_default: true,
                sample_rates: vec![16_000, 48_000],
            },
            DeviceInfo {
                name: "Yeti".into(),
                is_default: false,
                sample_rates: vec![],
            },
        ]
    }

    #[test]
    fn table_marks_the_default_device() {
        let table = render_table(&devices());
        let default_line = table
            .lines()
            .find(|line| line.contains("Микрофон"))
            .expect("the line of the default device");
        assert!(default_line.contains("yes"), "not marked: {default_line}");

        let other = table
            .lines()
            .find(|line| line.contains("Yeti"))
            .expect("the line of the second device");
        assert!(!other.contains("yes"), "extra mark: {other}");
    }

    #[test]
    fn table_lists_sample_rates_and_marks_unknown_ones() {
        let table = render_table(&devices());
        assert!(table.contains("16000, 48000"));
        assert!(table.contains('—'), "unknown rates are not marked");
    }

    #[test]
    fn json_output_is_an_array_of_devices() {
        let json = serde_json::to_string(&devices()).expect("serializes");
        let back: Vec<DeviceInfo> = serde_json::from_str(&json).expect("reads back");
        assert_eq!(back, devices());
    }
}
