use crate::error::{Error, Result};
use crate::model::{WindowsExperienceOptions, WindowsRegionalOptions};
use std::env;
use std::fs;

const COMPONENT: &str = "processorArchitecture=\"amd64\" publicKeyToken=\"31bf3856ad364e35\" language=\"neutral\" versionScope=\"nonSxS\" xmlns:wcm=\"http://schemas.microsoft.com/WMIConfig/2002/State\"";

pub(crate) fn answer_file(options: &WindowsExperienceOptions) -> Result<String> {
    validate(options)?;
    let mut settings = String::new();
    windows_pe(options, &mut settings);
    specialize(options, &mut settings);
    offline_servicing(options, &mut settings);
    oobe_system(options, &mut settings);
    Ok(format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<unattend xmlns=\"urn:schemas-microsoft-com:unattend\">\n{settings}</unattend>\n"
    ))
}

pub fn suggested_account_name() -> Option<String> {
    ["USER", "USERNAME"]
        .into_iter()
        .filter_map(|key| env::var(key).ok())
        .map(|value| value.trim().to_owned())
        .find(|value| valid_account_name(value))
}

pub fn host_regional_options() -> WindowsRegionalOptions {
    let locale = env::var("LC_ALL")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| env::var("LANG").ok())
        .map(|value| windows_locale(&value))
        .unwrap_or_else(|| "en-US".into());
    let iana_zone = env::var("TZ")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(system_iana_time_zone)
        .unwrap_or_else(|| "Etc/UTC".into());
    WindowsRegionalOptions {
        input_locale: locale.clone(),
        system_locale: locale.clone(),
        user_locale: locale.clone(),
        ui_language: locale,
        time_zone: windows_time_zone(&iana_zone).into(),
    }
}

fn validate(options: &WindowsExperienceOptions) -> Result<()> {
    if let Some(account) = options.local_account.as_deref()
        && !valid_account_name(account)
    {
        return Err(Error::UnsupportedImage(
            "Windows local-account name is empty, reserved, longer than 20 characters, or contains a forbidden character".into(),
        ));
    }
    if let Some(regional) = &options.regional {
        for (label, value) in [
            ("input locale", &regional.input_locale),
            ("system locale", &regional.system_locale),
            ("user locale", &regional.user_locale),
            ("UI language", &regional.ui_language),
            ("time zone", &regional.time_zone),
        ] {
            if value.trim().is_empty() || value.chars().any(char::is_control) {
                return Err(Error::UnsupportedImage(format!(
                    "Windows {label} must be a non-empty printable value"
                )));
            }
        }
    }
    Ok(())
}

fn valid_account_name(account: &str) -> bool {
    let trimmed = account.trim();
    let reserved = [
        "administrator",
        "guest",
        "defaultaccount",
        "wdagutilityaccount",
    ];
    !trimmed.is_empty()
        && trimmed == account
        && account.chars().count() <= 20
        && !account.ends_with('.')
        && !account
            .chars()
            .any(|character| character.is_control() || "\"/\\[]:;|=,+*?<>@&".contains(character))
        && !reserved
            .iter()
            .any(|reserved| account.eq_ignore_ascii_case(reserved))
}

fn windows_pe(options: &WindowsExperienceOptions, settings: &mut String) {
    if !options.bypass_hardware_requirements {
        return;
    }
    settings.push_str(&format!(
        "  <settings pass=\"windowsPE\">\n    <component name=\"Microsoft-Windows-Setup\" {COMPONENT}>\n      <RunSynchronous>\n"
    ));
    for (order, value) in ["BypassTPMCheck", "BypassSecureBootCheck", "BypassRAMCheck"]
        .into_iter()
        .enumerate()
    {
        settings.push_str(&format!(
            "        <RunSynchronousCommand wcm:action=\"add\"><Order>{}</Order><Path>reg add HKLM\\SYSTEM\\Setup\\LabConfig /v {value} /t REG_DWORD /d 1 /f</Path></RunSynchronousCommand>\n",
            order + 1
        ));
    }
    settings.push_str("      </RunSynchronous>\n    </component>\n  </settings>\n");
}

fn specialize(options: &WindowsExperienceOptions, settings: &mut String) {
    let mut commands = Vec::new();
    if options.allow_offline_account || options.local_account.is_some() {
        commands.push(
            "reg add &quot;HKLM\\Software\\Microsoft\\Windows\\CurrentVersion\\OOBE&quot; /v BypassNRO /t REG_DWORD /d 1 /f",
        );
    }
    if options.quality_of_life {
        commands.extend([
            "reg add &quot;HKLM\\Software\\Policies\\Microsoft\\Windows\\OneDrive&quot; /v DisableFileSyncNGSC /t REG_DWORD /d 1 /f",
            "reg add &quot;HKLM\\Software\\Policies\\Microsoft\\Windows\\WindowsCopilot&quot; /v TurnOffWindowsCopilot /t REG_DWORD /d 1 /f",
            "reg add &quot;HKLM\\Software\\Policies\\Microsoft\\Windows\\CloudContent&quot; /v DisableWindowsConsumerFeatures /t REG_DWORD /d 1 /f",
            "reg add &quot;HKLM\\Software\\Microsoft\\Windows\\CurrentVersion\\Communications&quot; /v ConfigureChatAutoInstall /t REG_DWORD /d 0 /f",
        ]);
    }
    if commands.is_empty() {
        return;
    }
    settings.push_str(&format!(
        "  <settings pass=\"specialize\">\n    <component name=\"Microsoft-Windows-Deployment\" {COMPONENT}>\n      <RunSynchronous>\n"
    ));
    for (order, command) in commands.into_iter().enumerate() {
        settings.push_str(&format!(
            "        <RunSynchronousCommand wcm:action=\"add\"><Order>{}</Order><Path>{command}</Path></RunSynchronousCommand>\n",
            order + 1
        ));
    }
    settings.push_str("      </RunSynchronous>\n    </component>\n  </settings>\n");
}

fn offline_servicing(options: &WindowsExperienceOptions, settings: &mut String) {
    if !options.disable_bitlocker && !options.force_s_mode {
        return;
    }
    settings.push_str("  <settings pass=\"offlineServicing\">\n");
    if options.disable_bitlocker {
        settings.push_str(&format!(
            "    <component name=\"Microsoft-Windows-SecureStartup-FilterDriver\" {COMPONENT}>\n      <PreventDeviceEncryption>true</PreventDeviceEncryption>\n    </component>\n    <component name=\"Microsoft-Windows-EnhancedStorage-Adm\" {COMPONENT}>\n      <TCGSecurityActivationDisabled>1</TCGSecurityActivationDisabled>\n    </component>\n"
        ));
    }
    if options.force_s_mode {
        settings.push_str(&format!(
            "    <component name=\"Microsoft-Windows-CodeIntegrity\" {COMPONENT}>\n      <SkuPolicyRequired>1</SkuPolicyRequired>\n    </component>\n"
        ));
    }
    settings.push_str("  </settings>\n");
}

fn oobe_system(options: &WindowsExperienceOptions, settings: &mut String) {
    let needs_shell = options.allow_offline_account
        || options.local_account.is_some()
        || options.regional.is_some()
        || options.minimize_data_collection
        || options.quality_of_life
        || options.apply_skusi_policy;
    if !needs_shell && !options.disable_bitlocker {
        return;
    }
    settings.push_str("  <settings pass=\"oobeSystem\">\n");
    if needs_shell {
        settings.push_str(&format!(
            "    <component name=\"Microsoft-Windows-Shell-Setup\" {COMPONENT}>\n"
        ));
        if options.allow_offline_account || options.minimize_data_collection {
            settings.push_str("      <OOBE>\n");
            if options.allow_offline_account {
                settings.push_str("        <HideOnlineAccountScreens>true</HideOnlineAccountScreens>\n        <HideWirelessSetupInOOBE>true</HideWirelessSetupInOOBE>\n");
            }
            if options.minimize_data_collection {
                settings.push_str(
                    "        <HideEULAPage>true</HideEULAPage>\n        <ProtectYourPC>3</ProtectYourPC>\n",
                );
            }
            settings.push_str("      </OOBE>\n");
        }
        if let Some(regional) = &options.regional {
            settings.push_str(&format!(
                "      <TimeZone>{}</TimeZone>\n",
                xml(&regional.time_zone)
            ));
        }
        if let Some(account) = options.local_account.as_deref() {
            let account = xml(account);
            settings.push_str(&format!(
                "      <UserAccounts>\n        <LocalAccounts>\n          <LocalAccount wcm:action=\"add\">\n            <Name>{account}</Name>\n            <DisplayName>{account}</DisplayName>\n            <Group>Administrators</Group>\n            <Password><Value>UABhAHMAcwB3AG8AcgBkAA==</Value><PlainText>false</PlainText></Password>\n          </LocalAccount>\n        </LocalAccounts>\n      </UserAccounts>\n"
            ));
        }
        let mut first_logon = Vec::new();
        if let Some(account) = options.local_account.as_deref() {
            first_logon.push(format!(
                "net user &quot;{}&quot; /logonpasswordchg:yes",
                xml(account)
            ));
            first_logon.push("net accounts /maxpwage:unlimited".into());
        }
        if options.apply_skusi_policy {
            first_logon.push("cmd /c mountvol S: /S &amp;&amp; copy %WINDIR%\\system32\\SecureBootUpdates\\SkuSiPolicy.p7b S:\\EFI\\Microsoft\\Boot &amp;&amp; mountvol S: /D".into());
        }
        if options.quality_of_life {
            first_logon.extend([
                "reg add &quot;HKLM\\System\\CurrentControlSet\\Control\\Session Manager\\Power&quot; /v HiberbootEnabled /t REG_DWORD /d 0 /f".into(),
                "reg add &quot;HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Advanced&quot; /v ShowCopilotButton /t REG_DWORD /d 0 /f".into(),
                "reg add &quot;HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\ContentDeliveryManager&quot; /v SystemPaneSuggestionsEnabled /t REG_DWORD /d 0 /f".into(),
                "reg add &quot;HKLM\\Software\\Policies\\Microsoft\\Edge&quot; /v HideFirstRunExperience /t REG_DWORD /d 1 /f".into(),
            ]);
        }
        if !first_logon.is_empty() {
            settings.push_str("      <FirstLogonCommands>\n");
            for (order, command) in first_logon.into_iter().enumerate() {
                settings.push_str(&format!(
                    "        <SynchronousCommand wcm:action=\"add\"><Order>{}</Order><CommandLine>{command}</CommandLine></SynchronousCommand>\n",
                    order + 1
                ));
            }
            settings.push_str("      </FirstLogonCommands>\n");
        }
        settings.push_str("    </component>\n");
    }
    if let Some(regional) = &options.regional {
        settings.push_str(&format!(
            "    <component name=\"Microsoft-Windows-International-Core\" {COMPONENT}>\n      <InputLocale>{}</InputLocale>\n      <SystemLocale>{}</SystemLocale>\n      <UserLocale>{}</UserLocale>\n      <UILanguage>{}</UILanguage>\n    </component>\n",
            xml(&regional.input_locale),
            xml(&regional.system_locale),
            xml(&regional.user_locale),
            xml(&regional.ui_language),
        ));
    }
    settings.push_str("  </settings>\n");
}

fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn windows_locale(locale: &str) -> String {
    let locale = locale.split_once('.').map_or(locale, |(locale, _)| locale);
    let locale = locale.split_once('@').map_or(locale, |(locale, _)| locale);
    if matches!(locale, "C" | "POSIX") {
        return "en-US".into();
    }
    let mut parts = locale.split(['_', '-']);
    let language = parts.next().unwrap_or("en").to_ascii_lowercase();
    match parts.next() {
        Some(region) if !region.is_empty() => {
            format!("{language}-{}", region.to_ascii_uppercase())
        }
        _ => language,
    }
}

fn system_iana_time_zone() -> Option<String> {
    let path = fs::read_link("/etc/localtime").ok()?;
    let path = path.to_string_lossy();
    path.split_once("/zoneinfo/")
        .map(|(_, zone)| zone.to_owned())
}

fn windows_time_zone(iana: &str) -> &'static str {
    match iana {
        "Asia/Kolkata" | "Asia/Calcutta" => "India Standard Time",
        "America/New_York" | "America/Toronto" => "Eastern Standard Time",
        "America/Chicago" => "Central Standard Time",
        "America/Denver" => "Mountain Standard Time",
        "America/Los_Angeles" | "America/Vancouver" => "Pacific Standard Time",
        "Europe/London" => "GMT Standard Time",
        "Europe/Paris" | "Europe/Berlin" | "Europe/Rome" | "Europe/Madrid" => {
            "W. Europe Standard Time"
        }
        "Europe/Helsinki" | "Europe/Kyiv" => "FLE Standard Time",
        "Asia/Tokyo" => "Tokyo Standard Time",
        "Asia/Shanghai" | "Asia/Hong_Kong" => "China Standard Time",
        "Asia/Singapore" => "Singapore Standard Time",
        "Australia/Sydney" => "AUS Eastern Standard Time",
        "Pacific/Auckland" => "New Zealand Standard Time",
        _ => "UTC",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_all_safe_windows_experience_sections() {
        let options = WindowsExperienceOptions {
            bypass_hardware_requirements: true,
            allow_offline_account: true,
            local_account: Some("Ada".into()),
            regional: Some(WindowsRegionalOptions {
                input_locale: "en-US".into(),
                system_locale: "en-US".into(),
                user_locale: "en-US".into(),
                ui_language: "en-US".into(),
                time_zone: "India Standard Time".into(),
            }),
            minimize_data_collection: true,
            disable_bitlocker: true,
            quality_of_life: true,
            use_windows_ca_2023: false,
            apply_skusi_policy: true,
            force_s_mode: true,
        };
        let answer = answer_file(&options).expect("answer file");
        for marker in [
            "BypassTPMCheck",
            "BypassNRO",
            "<Name>Ada</Name>",
            "India Standard Time",
            "ProtectYourPC",
            "PreventDeviceEncryption",
            "TurnOffWindowsCopilot",
            "SkuSiPolicy.p7b",
            "SkuPolicyRequired",
        ] {
            assert!(answer.contains(marker), "missing {marker}");
        }
        roxmltree::Document::parse(&answer).expect("valid XML");
    }

    #[test]
    fn rejects_reserved_or_injectable_account_names() {
        for account in ["Administrator", "two/users", " trailing", "name&evil"] {
            let options = WindowsExperienceOptions {
                local_account: Some(account.into()),
                ..WindowsExperienceOptions::default()
            };
            assert!(answer_file(&options).is_err(), "accepted {account}");
        }
    }

    #[test]
    fn translates_common_unix_locale_and_time_zone_values() {
        assert_eq!(windows_locale("en_US.UTF-8"), "en-US");
        assert_eq!(windows_locale("pt_BR@latin"), "pt-BR");
        assert_eq!(windows_time_zone("Asia/Kolkata"), "India Standard Time");
    }
}
