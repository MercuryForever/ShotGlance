//! 单实例保护 —— 第二个进程把第一个"叫到前面"然后自己退出。
//!
//! 为什么必须做：托盘常驻 + 全局热键的程序如果被重复双击，会出现
//! 两个托盘图标、两份 WebView2、两份 ONNX 会话；热键只有第一个注册成功，
//! 而用户点到的往往是第二个（它的热键没生效，看起来像"软件坏了"）。
//! 更糟的是两个实例会往同一份 `%APPDATA%\ShotGlance\` 写配置与模型下载，
//! 交错写能让 `cfg.json` 半截损坏。
//!
//! 实现选型（为什么不引 `tauri-plugin-single-instance`）：
//! 它在新机器上要求联网补 crate；而本仓库的交付包只带了一份
//! `cargo-registry.tar` 快照，多一个依赖就多一次离线失败的可能。
//! 这里要的功能只有两件事——"只准跑一个"和"把第一个叫出来"，
//! 用内核的命名互斥体 + 命名事件 80 行就够，且 `windows-sys`
//! 本来就在依赖树里（tauri 的传递依赖），零新增下载。
//!
//! 两个内核对象都放在 `Local\` 命名空间：作用域是"当前登录会话"。
//! 同一台机器上两个用户（或 RDP 会话）各跑一个实例是合理的，
//! 用 `Global\` 会把他们互相踢掉。
//!
//! 时序：
//! 1. `claim()` 在 `run()` 最前面调用，早于建窗、建托盘、注册热键。
//! 2. 首实例顺手把唤回事件建出来（窗口还没建就先备好），
//!    这样用户在启动过程中再双击一次，那个请求也不会丢——
//!    auto-reset 事件会把"已触发"这个状态保留到有人来 wait。
//! 3. `listen()` 在 setup 里起一个线程阻塞在 `WaitForSingleObject`，
//!    收到信号就通过 `run_on_main_thread` 回到主线程开面板。
//!
//! 唯一不生效的情况是"自检/取证进程"——见 `bypass()`。

use std::ffi::{c_void, OsStr};
use std::os::windows::ffi::OsStrExt;
use std::sync::atomic::{AtomicPtr, Ordering};

use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, WAIT_OBJECT_0,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, CreateMutexW, OpenEventW, SetEvent, WaitForSingleObject, EVENT_MODIFY_STATE,
    INFINITE,
};

use tauri::AppHandle;

/// 互斥体名字。带 `v1`：以后若要改语义（比如允许"多开但共享托盘"），
/// 换个名字就能让新旧版本互不干扰，不用背兼容包袱。
const MUTEX_NAME: &str = "Local\\ShotGlance.SingleInstance.Mutex.v1";

/// 唤回事件名字。第二个实例 `SetEvent` 它，第一个实例的监听线程被唤醒。
const EVENT_NAME: &str = "Local\\ShotGlance.SingleInstance.Event.v1";

/// 唤回事件句柄。`0` 表示"本进程不是首实例"或"事件没建出来"，
/// 两种情况下 `listen()` 都直接返回。
///
/// 用 `AtomicPtr` 而不是 `OnceLock<OwnedHandle>`：句柄本来就是裸指针，
/// 而且我们要**故意**在进程生命周期内不关闭它（退出时内核自动回收）。
/// 空指针表示"还没建"或"建失败"。
static WAKE_EVENT: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

/// 本进程在这次启动里的角色。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// 唯一实例，正常往下走。
    Primary,
    /// 已有实例在跑，且已经被通知过；本进程应当立刻退出。
    Secondary,
}

/// 这些环境变量代表"跑一次就退"的无界面工具，必须允许与托盘实例并存。
///
/// 不豁免的后果是静默失效：例如托盘实例开着时跑
/// `SHOTGLANCE_OCR_SELFTEST=1 shotglance.exe`，进程会被判成第二实例、
/// 直接退出，脚本拿不到结果还看不出原因（退出码 0，无任何输出）。
fn bypass() -> bool {
    const ONESHOT: [&str; 6] = [
        "SHOTGLANCE_SELF_TEST",     // 截图自检
        "SHOTGLANCE_OCR_SELFTEST",  // OCR 自检
        "SHOTGLANCE_MT_SELFTEST",   // 翻译自检
        "SHOTGLANCE_MT_BENCH",      // 解码基准
        "SHOTGLANCE_AUTO_OCR",      // 界面取证：自动跑一次识别
        "SHOTGLANCE_AUTO_OVERLAY",  // 界面取证：摆悬浮卡片
    ];
    ONESHOT.iter().any(|k| std::env::var_os(k).is_some())
}

/// 把 &str 转成以 0 结尾的宽字符，Win32 的 `*W` 系列只认这个。
fn wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

/// 抢占"唯一实例"身份。必须在 `tauri::Builder` 之前调用。
pub fn claim() -> Role {
    if bypass() {
        return Role::Primary;
    }

    let mutex_name = wide(MUTEX_NAME);
    // `bInitialOwner = FALSE`：这里只关心"这个名字有没有人建过"，
    // 不靠所有权做互斥。用所有权的话，首实例一旦意外退出，
    // 互斥体会变成 abandoned 状态，判断反而绕。
    let mutex: HANDLE = unsafe { CreateMutexW(std::ptr::null(), 0, mutex_name.as_ptr()) };
    if mutex.is_null() {
        // 建不出内核对象只可能是沙箱/权限异常（正常用户态进程不会失败）。
        // 此时选择放行：少一层保护，好过软件直接打不开。
        eprintln!(
            "[shotglance] 单实例互斥体创建失败，按唯一实例继续: {}",
            std::io::Error::last_os_error()
        );
        return Role::Primary;
    }

    // 注意必须紧接着取 GetLastError：`CreateMutexW` 成功时也可能留下
    // 上一次调用的错误码，中间插任何别的 Win32 调用都会把它冲掉。
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        wake_primary();
        unsafe { CloseHandle(mutex) };
        return Role::Secondary;
    }

    // 首实例：立刻把唤回事件备好，别等到 setup——中间这段空窗期
    // 用户再双击一次的话，第二个进程会 SetEvent 落空，那个请求就丢了。
    let event_name = wide(EVENT_NAME);
    // `bManualReset = FALSE`：auto-reset。一次信号唤醒一次等待，
    // 连续双击两次就排队两次，不会粘住导致每轮 wait 都立刻返回。
    let event: HANDLE = unsafe { CreateEventW(std::ptr::null(), 0, 0, event_name.as_ptr()) };
    if event.is_null() {
        eprintln!(
            "[shotglance] 唤回事件创建失败，第二个实例将只能退出、无法唤回面板: {}",
            std::io::Error::last_os_error()
        );
    } else {
        WAKE_EVENT.store(event, Ordering::SeqCst);
    }

    // `mutex` 句柄故意不关：它活着，"这个名字已被占用"这个事实才成立。
    // 进程退出时内核自动回收，不需要也来不及手动 CloseHandle。
    Role::Primary
}

/// 第二个进程：叫醒已经在跑的那个。
fn wake_primary() {
    let event_name = wide(EVENT_NAME);
    let event: HANDLE = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, event_name.as_ptr()) };
    if event.is_null() {
        eprintln!(
            "[shotglance] 已有实例在运行，但唤回事件打不开（可能权限不同），直接退出: {}",
            std::io::Error::last_os_error()
        );
        return;
    }
    unsafe {
        SetEvent(event);
        CloseHandle(event);
    }
    eprintln!("[shotglance] 已有实例在运行，已请求它前置面板，本进程退出");
}

/// 起一个线程等"唤回"信号。只对首实例有效，其它情况立刻返回。
///
/// 放在 `setup` 里调：那时 `AppHandle` 可用，且主线程的事件循环即将启动，
/// `run_on_main_thread` 投递的闭包一定有人执行。
pub fn listen(app: AppHandle) {
    // 句柄是指针，指针不是 Send，跨不到线程里去；转成 usize 再在闭包里转回来。
    // 这不是随手 hack：句柄值本身就是个不透明整数，进程内全局唯一且不会失效
    // （我们持有着它，没有 CloseHandle）。
    let event = WAKE_EVENT.load(Ordering::SeqCst) as usize;
    if event == 0 {
        return;
    }

    std::thread::spawn(move || loop {
        let rc = unsafe { WaitForSingleObject(event as HANDLE, INFINITE) };
        if rc != WAIT_OBJECT_0 {
            // 只在真出错时退出循环，避免变成忙等把 CPU 打满。
            eprintln!("[shotglance] 单实例唤回等待失败（{rc}），停止监听");
            return;
        }
        let app_for_main = app.clone();
        // 面板只能在主线程建/显示。这里从工作线程投递回去，
        // 顺带把"叫醒"和"建窗"两件事串起来：open_panel 自带
        // 复用/退出悬浮形态/前置 三件事，不需要另写一套。
        let _ = app.run_on_main_thread(move || {
            if let Err(e) = super::open_panel(&app_for_main) {
                eprintln!("[shotglance] 唤回面板失败: {e}");
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 宽字符串必须补 0 结尾，少这一个 0 会读到栈上的随机内存。
    #[test]
    fn wide_appends_nul() {
        let w = wide("ab");
        assert_eq!(w, vec![b'a' as u16, b'b' as u16, 0]);
    }

    /// 两个内核对象要落在 Local\ 命名空间，否则跨会话（RDP）会互相踢。
    #[test]
    fn names_are_session_local() {
        assert!(MUTEX_NAME.starts_with("Local\\"));
        assert!(EVENT_NAME.starts_with("Local\\"));
        assert_ne!(MUTEX_NAME, EVENT_NAME);
    }

    /// 自检开关必须能绕过单实例，否则那些脚本会被判成第二实例静默退出。
    #[test]
    fn oneshot_env_bypasses_guard() {
        // 同一进程里改环境变量会串到别的测试，用一次性子进程验证太重，
        // 这里只断言"没有任何自检变量时不该豁免"这个默认值。
        if std::env::var_os("SHOTGLANCE_SELF_TEST").is_none()
            && std::env::var_os("SHOTGLANCE_OCR_SELFTEST").is_none()
            && std::env::var_os("SHOTGLANCE_MT_SELFTEST").is_none()
            && std::env::var_os("SHOTGLANCE_MT_BENCH").is_none()
            && std::env::var_os("SHOTGLANCE_AUTO_OCR").is_none()
            && std::env::var_os("SHOTGLANCE_AUTO_OVERLAY").is_none()
        {
            assert!(!bypass(), "干净环境下不应豁免单实例保护");
        }
    }
}
