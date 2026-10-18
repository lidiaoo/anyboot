//! 电源动作（重启 / 关机 / 睡眠）+ 并发护栏（POWER_RELEASE_*）。
//! RemoteNoGuiAuth 在这里；远程 GUI 认证定则见 docs/server-hub.md。

use super::*;

/// 「已提交电源动作」之后忙闸最多挂多久（毫秒）。
///
/// 忙闸的用意是「别重复提交」，但**提交成功 ≠ 动作真的发生**：`systemctl suspend`
/// 在 logind 那边只是**排队**，命令立刻返回 0；随后被 inhibitor 拦下、或内核
/// suspend 失败立刻 resume，机器照样活着 —— 而闸一直挂着，三个电源项就永远置灰、
/// 点了毫无反应（用户报的「三个电源按钮都没用」就是这么来的：点过睡眠/被拦下的重启
/// 之后，后面每一次点击都被静默吞掉，只能重启应用才恢复）。
/// 真关机/重启了的话没人会看菜单，放掉闸没有任何代价。
pub(crate) const POWER_RELEASE_MS: u64 = 15_000;

/// 放闸的世代号：新动作一来就把先前排队的放闸作废（与 schedule_clamp /
/// schedule_center_snap 同一套「世代号 + 静默期」写法）。
pub(crate) static POWER_RELEASE_GENERATION: AtomicU64 = AtomicU64::new(0);

/// 抢占一个电源动作；已经有动作在跑时返回 false（替代原来的静态原子标志）。
pub(crate) fn try_begin_power(app: &AppHandle, action: &str) -> bool {
    let acquired = {
        let state = app.state::<Mutex<BusyState>>();
        let mut busy = state.lock().expect("busy mutex poisoned");
        if busy.power.is_some() {
            false
        } else {
            busy.power = Some(action.to_string());
            // 新动作一来就把先前排队的放闸作废。**必须在这把锁里推高**：放闸那边
            // 也是在同一把锁里比对世代号（见 schedule_power_release），两边同锁才
            // 不会出现「新动作刚置上 busy、世代号还没换，旧放闸把它的闸清掉」的缝。
            POWER_RELEASE_GENERATION.fetch_add(1, Ordering::SeqCst);
            true
        }
    };
    if acquired {
        publish_ui(app);
    } else {
        trace(
            app,
            "power",
            &format!("已有电源动作在处理中，忽略 {action}"),
        );
        // 被忙闸挡掉的点击**也必须留痕**：从前这里只写 trace，而 trace 默认是关的，
        // 于是「点了没反应」在现场等于彻底无声 —— 正是用户这次报障最难查的地方。
        record_action(
            app,
            "电源操作",
            false,
            "上一个电源动作还没结束，这次点击已忽略；若菜单里的电源项一直是灰的，就是它卡住了",
        );
        publish_ui(app);
    }
    acquired
}

/// 电源动作结束（失败/被拒绝时调用，让按钮重新可用；成功时系统马上就没了）。
pub(crate) fn finish_power(app: &AppHandle) {
    {
        let state = app.state::<Mutex<BusyState>>();
        let mut busy = state.lock().expect("busy mutex poisoned");
        busy.power = None;
    }
    publish_ui(app);
}

/// 提交成功之后**延时放闸**（见 POWER_RELEASE_MS）。
///
/// 到点时若世代号还是最新、且忙闸确实还挂着，就放掉 —— 说明「命令返回成功」之后
/// 系统并没有真的关机/重启/睡着，别让三个电源项永远灰着。
/// 线程睡眠走 CLOCK_MONOTONIC（不含 suspend 时间），所以真睡了一觉回来之后，
/// 剩下的静默期照样会走完、闸照样放掉 —— 正是我们要的。
pub(crate) fn schedule_power_release(app: &AppHandle) {
    let generation = POWER_RELEASE_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(POWER_RELEASE_MS));
        // 「世代号还是不是最新」与「清掉 busy」必须在**同一把锁**里做：
        // 否则新动作刚在 try_begin_power 里置上 busy、世代号还没换的那一刻，
        // 这里会把它的闸放掉（新动作就没人看了）。
        {
            let state = app.state::<Mutex<BusyState>>();
            let mut busy = state.lock().expect("busy mutex poisoned");
            if POWER_RELEASE_GENERATION.load(Ordering::SeqCst) != generation
                || busy.power.is_none()
            {
                return;
            }
            busy.power = None;
        }
        trace(&app, "power", "提交后系统仍在：放掉忙闸，电源项恢复可用");
        record_action(
            &app,
            "电源操作",
            false,
            "已提交的动作在 15 秒内没有关机/重启/睡眠，电源项已恢复可用（真在关机中就不用管）",
        );
        publish_ui(&app);
    });
}

/// 电源动作：重启 / 关机 / 睡眠。缺省是重启（HTTP / CLI 不传就是它）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PowerAction {
    Reboot,
    Shutdown,
    Sleep,
}

impl Default for PowerAction {
    fn default() -> Self {
        Self::Reboot
    }
}

impl PowerAction {
    /// 解析动作名：大小写不敏感，同时接受常见别名。
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "reboot" | "restart" => Some(Self::Reboot),
            "shutdown" | "poweroff" | "halt" => Some(Self::Shutdown),
            "sleep" | "suspend" => Some(Self::Sleep),
            _ => None,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Reboot => "reboot",
            Self::Shutdown => "shutdown",
            Self::Sleep => "sleep",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Reboot => "重启系统",
            Self::Shutdown => "关机",
            Self::Sleep => "睡眠",
        }
    }

    /// 提交成功后的回执文案。
    pub(crate) fn submitted_message(self) -> &'static str {
        match self {
            Self::Reboot => "已提交重启，系统即将重启",
            Self::Shutdown => "已提交关机，系统即将关机",
            Self::Sleep => "已提交睡眠",
        }
    }
}

/// 一条候选命令。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PowerAttempt {
    pub(crate) program: &'static str,
    pub(crate) args: Vec<String>,
    /// 需要本机图形授权（macOS 管理员框 / Linux pkexec）：
    /// 远程与 CLI 默认整条跳过（Q3），只有本机点击才允许弹出
    pub(crate) needs_gui_auth: bool,
}

impl PowerAttempt {
    pub(crate) fn command(&self) -> String {
        render_command(self.program, &self.args)
    }
}

pub(crate) fn render_command(program: &str, args: &[String]) -> String {
    if args.is_empty() {
        program.to_string()
    } else {
        format!("{program} {}", args.join(" "))
    }
}

/// 一次候选命令执行的结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PowerAttemptOutcome {
    /// 命令执行成功，动作已提交。
    Success,
    /// 系统里没有这个命令，可以换下一个候选。
    Missing(String),
    /// 命令在，但**这条候选在本机用不了**（典型：`sudo -n` 没配免密 sudoers、
    /// macOS 的「自动化」授权被拒）。
    ///
    /// 必须和 `Missing` 一样按「换下一条候选」处理：它只是「这条方式行不通」，
    /// 不是「用户拒绝了这次操作」。若归进 `Failed`，macOS 上会直接 Stop
    /// （`escalate_power` 在 macOS 恒返回 None）→ 后面那条管理员授权候选永远跑不到。
    Unavailable(String),
    /// 命令存在但执行失败，通常是权限不足或用户取消了授权弹窗。
    Failed(String),
}

impl PowerAttemptOutcome {
    pub(crate) fn describe(&self) -> String {
        match self {
            Self::Success => "已提交".to_string(),
            Self::Missing(reason) | Self::Unavailable(reason) | Self::Failed(reason) => {
                reason.clone()
            }
        }
    }
}

/// 一条尝试失败之后该做什么。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PowerStep {
    /// 动作已提交，结束。
    Done,
    /// 换下一个候选命令（例如发行版里没有 systemctl）。
    Next,
    /// 权限不足，用 pkexec 提权重试同一条命令。
    Escalate,
    /// 停手：命令能跑但失败了，多半是用户取消了授权，不能换个方式照样执行。
    Stop,
}

pub(crate) fn next_power_step(outcome: &PowerAttemptOutcome, can_escalate: bool) -> PowerStep {
    match outcome {
        PowerAttemptOutcome::Success => PowerStep::Done,
        // 「没有这个命令」和「这条候选在本机用不了」都是换下一条，不是停手。
        PowerAttemptOutcome::Missing(_) | PowerAttemptOutcome::Unavailable(_) => PowerStep::Next,
        PowerAttemptOutcome::Failed(_) if can_escalate => PowerStep::Escalate,
        PowerAttemptOutcome::Failed(_) => PowerStep::Stop,
    }
}

/// 各平台的候选命令，按顺序尝试（见 plan §3.2）。
///
/// macOS 先走 **AppleEvent → System Events**：这是苹果菜单里「重新启动/关机/睡眠」
/// 的同一条链路（`loginwindow` 的 `aevt`/`rst`/`sdn` 事件），**任何用户都不需要
/// 管理员密码**，也不需要改任何配置文件；代价只是首次弹一次「自动化」授权
/// （「AnyBoot」想要控制「System Events」），点允许后写进该用户的 TCC 库，此后永久免框。
/// 用不了（系统里没授权、缺 osascript）才退到免密 sudo，最后才是管理员授权框。
/// Linux 先试免密的 systemctl（本地活动会话下 logind 允许），失败再走 pkexec；
/// Windows 用自带的 shutdown / rundll32。
#[cfg(target_os = "macos")]
pub(crate) fn power_attempts(action: PowerAction) -> Vec<PowerAttempt> {
    /// 走系统管理员授权框执行一条 shell 命令。
    fn admin_script(command: &str) -> PowerAttempt {
        let script = format!("do shell script \"{command}\" with administrator privileges");
        attempt("osascript", &["-e", &script], true)
    }

    /// 免密 sudo：`-n` 保证**绝不问密码** —— 配了免密 sudoers 就直接执行成功（不弹框），
    /// 没配就立刻失败并被判为 `Unavailable` → 换下一条（管理员授权框）。
    /// 也就是「配了免密才免框，没配的机器行为与以前完全一样」。
    fn sudo_nopasswd(arguments: &[&'static str]) -> PowerAttempt {
        let mut args = vec!["-n"];
        args.extend(arguments);
        attempt("sudo", &args, false)
    }

    /// AppleEvent 到 System Events（= 苹果菜单的同一条路，免 root、免配置）。
    ///
    /// `needs_gui_auth` 记 false：授权是一次性的**状态**（TCC 库），拿到之后
    /// 远程/CLI 调也一样免框；只有从未授权过的那一次会弹本机授权框
    /// （与睡眠那条候选同一约定，见 `docs/remote-api-plan.md` §3.2）。
    /// macOS 10.14+ 要求 app 声明 `NSAppleEventsUsageDescription`
    /// （`src-tauri/Info.plist`），否则**连授权框都不弹**、直接报 -1743。
    fn apple_event(verb: &str) -> PowerAttempt {
        let script = format!("tell application \"System Events\" to {verb}");
        attempt("osascript", &["-e", &script], false)
    }

    match action {
        PowerAction::Reboot => vec![
            apple_event("restart"),
            sudo_nopasswd(&["/sbin/shutdown", "-r", "now"]),
            admin_script("/sbin/shutdown -r now"),
        ],
        PowerAction::Shutdown => vec![
            apple_event("shut down"),
            sudo_nopasswd(&["/sbin/shutdown", "-h", "now"]),
            admin_script("/sbin/shutdown -h now"),
        ],
        PowerAction::Sleep => vec![
            apple_event("sleep"),
            // 配了免密 sudo 的机器：这条就成功，不用再弹框
            sudo_nopasswd(&["/usr/bin/pmset", "sleepnow"]),
            // 兜底：管理员授权框
            admin_script("/usr/bin/pmset sleepnow"),
        ],
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn power_attempts(action: PowerAction) -> Vec<PowerAttempt> {
    match action {
        PowerAction::Reboot => vec![
            attempt("systemctl", &["reboot"], false),
            // 没有 systemd 的发行版（elogind 等）用 loginctl。
            attempt("loginctl", &["reboot"], false),
        ],
        PowerAction::Shutdown => vec![
            attempt("systemctl", &["poweroff"], false),
            attempt("loginctl", &["poweroff"], false),
        ],
        PowerAction::Sleep => vec![
            attempt("systemctl", &["suspend"], false),
            attempt("loginctl", &["suspend"], false),
        ],
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn power_attempts(action: PowerAction) -> Vec<PowerAttempt> {
    match action {
        PowerAction::Reboot => vec![attempt("shutdown", &["/r", "/t", "0"], false)],
        PowerAction::Shutdown => vec![attempt("shutdown", &["/s", "/t", "0"], false)],
        // 系统启用休眠时 Windows 会休眠而不是睡眠：只读预检会如实说明（Q10′ 方案 C）
        PowerAction::Sleep => vec![attempt(
            "rundll32.exe",
            &["powrprof.dll,SetSuspendState", "0,1,0"],
            false,
        )],
    }
}

/// 需要管理员权限时的等价命令：Linux 用 pkexec 弹图形授权框。
/// 其他平台没有额外方案（macOS 的候选命令本身就是提权的）。
pub(crate) fn escalate_power(attempt: &PowerAttempt, allow_gui_auth: bool) -> Option<PowerAttempt> {
    #[cfg(target_os = "linux")]
    {
        if is_root() || attempt.needs_gui_auth || !allow_gui_auth {
            return None;
        }
        let mut args = Vec::with_capacity(attempt.args.len() + 1);
        args.push(attempt.program.to_string());
        args.extend(attempt.args.iter().cloned());
        Some(PowerAttempt {
            program: "pkexec",
            args,
            needs_gui_auth: true,
        })
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (attempt, allow_gui_auth);
        None
    }
}

/// 真的去执行一条候选命令（dry-run 时不会走到这里）。
/// `sudo -n`（免密模式）在**没有配免密 sudoers** 时的失败文案。
///
/// 这几种都不是「用户取消了授权框」，而是「这条候选在本机用不了」——
/// `-n` 的语义就是「绝不问密码」，所以它失败时机器上根本没人被问过。
/// 判定做成纯函数，单测能钉死文案（sudo 的原文随版本略有差别，多认几种）。
pub(crate) fn sudo_without_password(program: &str, args: &[String], stderr: &str) -> bool {
    program == "sudo"
        && args.iter().any(|argument| argument == "-n")
        && [
            "a password is required",
            "no tty present",
            "a terminal is required",
        ]
        .iter()
        .any(|needle| stderr.contains(needle))
}

/// AppleEvents（「自动化」授权）被拒时 osascript 的失败文案。
///
/// 和 `sudo -n` 一样属于「这条候选在本机用不了」，不算「用户当场取消了授权框」：
/// TCC 的拒绝是一个**持久状态**（拒了就一直是拒的，不会这次拒下次又问），
/// 所以归 `Unavailable` 换下一条候选，而不是 `Failed` 停手 —— 停手等于把
/// 整台机器的重启/关机按钮打死，用户连输密码那条路都走不到。
/// 判定做成纯函数，单测能钉死文案（osascript 的报错会随系统语言变，按错误码为主）。
pub(crate) fn apple_events_denied(program: &str, args: &[String], stderr: &str) -> bool {
    program == "osascript"
        && args.iter().any(|argument| argument.contains("System Events"))
        && [
            "-1743", // errAEEventNotPermitted
            "Not authorized to send Apple events",
            "not allowed to send Apple events",
        ]
        .iter()
        .any(|needle| stderr.contains(needle))
}

pub(crate) fn run_power_command(program: &str, args: &[String]) -> PowerAttemptOutcome {
    let display = render_command(program, args);
    match command(program).args(args).output() {
        Ok(output) if output.status.success() => PowerAttemptOutcome::Success,
        Ok(output) => {
            let code = output
                .status
                .code()
                .map(|code| code.to_string())
                .unwrap_or_else(|| "未知".to_string());
            let stderr = String::from_utf8_lossy(&output.stderr);
            let detail = stderr
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            // 免密 sudo 没配 → 换下一条候选（管理员授权框），别当作用户取消授权。
            if sudo_without_password(program, args, &detail) {
                return PowerAttemptOutcome::Unavailable(format!("{display}：{detail}"));
            }
            // 「自动化」没授权 → 同上，换下一条候选。
            if apple_events_denied(program, args, &detail) {
                return PowerAttemptOutcome::Unavailable(format!("{display}：{detail}"));
            }
            if detail.is_empty() {
                PowerAttemptOutcome::Failed(format!("{display} 失败（退出码 {code}）"))
            } else {
                PowerAttemptOutcome::Failed(format!("{display} 失败（退出码 {code}）：{detail}"))
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            PowerAttemptOutcome::Missing(format!("{display}：{error}"))
        }
        Err(error) => PowerAttemptOutcome::Failed(format!("{display} 无法执行: {error}")),
    }
}

/// 一次电源动作的执行回执。
#[derive(Clone, Debug)]
pub(crate) struct PowerReport {
    /// dry-run：只列出「将会执行」的命令，没有真的执行
    pub(crate) dry_run: bool,
    /// 实际尝试过的命令（dry-run 时是将会执行的命令）
    pub(crate) commands: Vec<String>,
}

/// dry-run 探针：`ANYBOOT_POWER_DRY_RUN=1` 时只打印不执行。
///
/// 本机验证电源动作不能真的把机器关掉，这个开关让「三端命令表 + 置灰 + 回执」
/// 全部可以反复自动验证。
pub(crate) fn power_dry_run() -> bool {
    env_flag_from(std::env::var("ANYBOOT_POWER_DRY_RUN").ok().as_deref()).unwrap_or(false)
}

/// 只渲染「将会执行」的命令清单（dry-run 与日志共用，不执行任何东西）。
pub(crate) fn plan_commands(attempts: &[PowerAttempt], allow_gui_auth: bool) -> Vec<String> {
    let mut plan = Vec::new();
    for candidate in attempts {
        if candidate.needs_gui_auth && !allow_gui_auth {
            plan.push(format!(
                "{}（需要本机授权，远程/CLI 会跳过）",
                candidate.command()
            ));
            continue;
        }
        plan.push(candidate.command());
        if let Some(escalated) = escalate_power(candidate, allow_gui_auth) {
            plan.push(format!(
                "{}（仅当上一条因权限失败时才用）",
                escalated.command()
            ));
        }
    }
    plan
}

/// 执行电源动作。
///
/// dry-run 时只回传命令清单；真跑时按候选表顺序尝试，沿用既有安全约定——
/// **只有「命令不存在」和「这条候选在本机用不了」才换下一条候选**，
/// 命令能跑但失败时要么提权重试、要么停手（用户当场取消了授权框就绝不再换方式执行）。
pub(crate) fn run_power_action(
    action: PowerAction,
    allow_gui_auth: bool,
    dry_run: bool,
) -> Result<PowerReport, String> {
    let attempts = power_attempts(action);
    if dry_run {
        return Ok(PowerReport {
            dry_run: true,
            commands: plan_commands(&attempts, allow_gui_auth),
        });
    }

    let mut failures = Vec::new();
    let mut commands = Vec::new();
    for candidate in attempts {
        if candidate.needs_gui_auth && !allow_gui_auth {
            failures.push(format!("{}（需要本机授权，已跳过）", candidate.command()));
            continue;
        }
        let outcome = run_power_command(&candidate.program, &candidate.args);
        commands.push(candidate.command());
        let escalation = escalate_power(&candidate, allow_gui_auth);
        match next_power_step(&outcome, escalation.is_some()) {
            PowerStep::Done => {
                return Ok(PowerReport {
                    dry_run: false,
                    commands,
                })
            }
            PowerStep::Next => {
                failures.push(outcome.describe());
                continue;
            }
            PowerStep::Escalate => {
                let Some(escalated) = escalation else {
                    return Err(format!("{}失败：{}", action.label(), outcome.describe()));
                };
                let escalated_outcome = run_power_command(&escalated.program, &escalated.args);
                commands.push(escalated.command());
                return match escalated_outcome {
                    PowerAttemptOutcome::Success => Ok(PowerReport {
                        dry_run: false,
                        commands,
                    }),
                    other => Err(format!(
                        "{}需要管理员授权，提权未完成：{}",
                        action.label(),
                        other.describe()
                    )),
                };
            }
            PowerStep::Stop => {
                return Err(format!(
                    "{}失败：{}（可能是没有权限或取消了授权）",
                    action.label(),
                    outcome.describe()
                ))
            }
        }
    }
    Err(format!(
        "没有可用的{}命令（已尝试：{}）",
        action.label(),
        failures.join("；")
    ))
}

/// Windows 休眠是否启用（Q10′ 方案 C 的只读预检结论）。
#[cfg(any(target_os = "windows", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HibernationState {
    /// 休眠已启用 → Windows 会把「睡眠」变成休眠
    Enabled,
    /// 休眠已关闭 → 「睡眠」会真的进入睡眠（S3）
    Disabled,
    /// 读不到也说不出，按未知处理（不猜）
    Unknown,
}

/// 读注册表 `HKLM\SYSTEM\...\Control\Power\HibernateEnabled`（语言无关）。
#[cfg(target_os = "windows")]
pub(crate) fn read_hibernate_enabled_registry() -> Option<bool> {
    use winreg::enums::HKEY_LOCAL_MACHINE;
    use winreg::RegKey;

    let key = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey(r"SYSTEM\CurrentControlSet\Control\Power")
        .ok()?;
    let value: u32 = key.get_value("HibernateEnabled").ok()?;
    Some(value != 0)
}

/// 睡眠之前做一次只读预检，返回「给用户看的一句话」+「证据（写进 trace）」。
pub(crate) fn sleep_probe() -> (Option<String>, Vec<String>) {
    #[cfg(target_os = "windows")]
    {
        let mut evidence = Vec::new();
        let state = match read_hibernate_enabled_registry() {
            Some(enabled) => {
                evidence.push(format!("注册表 HibernateEnabled={}", u8::from(enabled)));
                if enabled {
                    HibernationState::Enabled
                } else {
                    HibernationState::Disabled
                }
            }
            None => {
                evidence.push("注册表 HibernateEnabled 读取失败，改用 powercfg /a".to_string());
                match command("powercfg").arg("/a").output() {
                    Ok(output) => {
                        let raw = String::from_utf8_lossy(&output.stdout).to_string();
                        evidence.push(format!(
                            "powercfg /a 原文: {}",
                            raw.replace(['\r', '\n'], " ").trim()
                        ));
                        hibernation_from_powercfg_output(&raw)
                    }
                    Err(error) => {
                        evidence.push(format!("powercfg /a 无法执行: {error}"));
                        HibernationState::Unknown
                    }
                }
            }
        };
        let note = match state {
            HibernationState::Enabled => {
                "注意：系统启用了休眠，Windows 会把这次「睡眠」变成休眠（要真睡眠先执行 powercfg /hibernate off）"
                    .to_string()
            }
            HibernationState::Disabled => "本次会进入睡眠（S3）".to_string(),
            HibernationState::Unknown => {
                "未能判定这次是睡眠还是休眠，见 trace 里 powercfg /a 的原文".to_string()
            }
        };
        (Some(note), evidence)
    }
    #[cfg(not(target_os = "windows"))]
    {
        (None, Vec::new())
    }
}

/// 后台执行电源动作，结果写进快照推给主窗口。
///
/// 提权弹窗可能停很久，所以不能在命令线程里同步跑；重复请求由 busy.power 挡掉
/// （托盘项和主窗口按钮会按同一份 busy 同时置灰）。
///
/// `allow_gui_auth`：本机点击传 true（可以弹系统授权框）；阶段 3 的 HTTP 与
/// 阶段 5 的 CLI 传 false（Q3：远程不弹本机授权框，直接说明原因）。
pub(crate) fn spawn_power(app: &AppHandle, action: PowerAction, allow_gui_auth: bool) {
    if !try_begin_power(app, action.as_str()) {
        return;
    }
    trace(
        app,
        "power",
        &format!(
            "开始执行 {}（dry_run={} allow_gui_auth={}）",
            action.as_str(),
            power_dry_run(),
            allow_gui_auth
        ),
    );
    // 睡眠前做只读预检：Windows 上判断这次是真睡眠（S3）还是会变成休眠（Q10′ 方案 C）
    let (note, evidence) = if action == PowerAction::Sleep {
        sleep_probe()
    } else {
        (None, Vec::new())
    };
    for line in &evidence {
        trace(app, "power", line);
    }

    let dry_run = power_dry_run();
    let app = app.clone();
    std::thread::spawn(
        move || match run_power_action(action, allow_gui_auth, dry_run) {
            Ok(report) => {
                let message = if report.dry_run {
                    format!(
                        "dry-run：未真的执行，将按顺序尝试 {}",
                        report.commands.join(" → ")
                    )
                } else if let Some(note) = &note {
                    format!("{}（{note}）", action.submitted_message())
                } else {
                    action.submitted_message().to_string()
                };
                trace(
                    &app,
                    "power",
                    &format!(
                        "{} 成功 dry_run={} commands={:?}",
                        action.as_str(),
                        report.dry_run,
                        report.commands
                    ),
                );
                record_action(&app, action.label(), true, &message);
                if report.dry_run {
                    // dry-run 没真的执行：立刻解除 busy，方便继续验证
                    finish_power(&app);
                } else {
                    // 成功时系统马上就没了：保留 busy，避免用户再点一次。
                    // 但「命令返回成功」不等于动作真的发生（见 POWER_RELEASE_MS），
                    // 所以排一次延时放闸，别让电源项永远灰着。
                    schedule_power_release(&app);
                    publish_ui(&app);
                }
            }
            Err(error) => {
                trace(&app, "power", &format!("{} 失败: {error}", action.as_str()));
                record_action(&app, action.label(), false, &error);
                // 失败时解除 busy，允许再试一次（finish_power 内部会发布快照）
                finish_power(&app);
                // 从托盘触发时主窗口可能没打开，显示出来让用户看到失败原因。
                // 窗口/激活策略属于 AppKit 主线程操作，这里显式切回主线程。
                let window_app = app.clone();
                if let Err(error) = app
                    .run_on_main_thread(move || show_main_window(&window_app, ShowOrigin::Recall))
                {
                    trace(&app, "power", &format!("显示主窗口失败: {error}"));
                }
            }
        },
    );
}

/// 远程/CLI 请求正在执行：这段时间内**不弹**本机授权框（Q3 定稿 —— 远程请求不能
/// 挂在那里等人去点确认）。需要 root 的步骤直接失败并说清该怎么办。
///
/// 为什么用全局量而不是逐层传参：策略入口只有 `execute_remote_command` 一个，但它
/// 触发的一次扫描要穿过 rescan_grub → grub_scan → grub_collect 三层，中间还夹着
/// 本机动作共用的 `refresh_and_update` —— 传参会把本机那条路也污染一遍。
pub(crate) static REMOTE_NO_GUI_AUTH: AtomicBool = AtomicBool::new(false);

/// 作用域守卫：把一次远程请求圈起来（panic 也不会漏关）。
pub(crate) struct RemoteNoGuiAuth;

impl RemoteNoGuiAuth {
    /// 只有「本次请求不允许本机授权」时才需要圈。
    pub(crate) fn enter(allow_gui_auth: bool) -> Option<Self> {
        if allow_gui_auth {
            return None;
        }
        REMOTE_NO_GUI_AUTH.store(true, Ordering::SeqCst);
        Some(Self)
    }
}

impl Drop for RemoteNoGuiAuth {
    fn drop(&mut self) {
        REMOTE_NO_GUI_AUTH.store(false, Ordering::SeqCst);
    }
}

/// 需要 root 但当前策略不许弹授权框时的统一说法（远程改不动就明说改不动）。
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn gui_auth_forbidden() -> Option<String> {
    REMOTE_NO_GUI_AUTH.load(Ordering::SeqCst).then(|| {
        "需要本机授权（远程请求默认不弹授权框）：请在目标机主窗口点一次「识别」，\
         或打开 remoteAllowGuiAuth，或把 ESP 挂成当前用户可写（见 README 权限说明）"
            .to_string()
    })
}
