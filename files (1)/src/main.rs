#![windows_subsystem = "windows"]

use eframe::egui::{self, Color32, RichText};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::process::{Command, Stdio};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

type Res<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

const MC: &str = "1.21.11";
const ACCENT: Color32 = Color32::from_rgb(139, 92, 246);
const BG: Color32 = Color32::from_rgb(15, 12, 26);
const CARD: Color32 = Color32::from_rgb(26, 21, 48);
const MUTED: Color32 = Color32::from_rgb(157, 147, 196);

const TOKEN_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/token";
const DEVICE_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode";
const SCOPE: &str = "XboxLive.signin offline_access";

const PERF_MODS: &[&str] = &[
    "fabric-api", "sodium", "lithium", "ferrite-core", "immediatelyfast",
    "entityculling", "modernfix", "krypton", "sodium-extra",
];

const JVM_FLAGS: &[&str] = &[
    "-XX:+UseG1GC", "-XX:+ParallelRefProcEnabled", "-XX:MaxGCPauseMillis=200",
    "-XX:+UnlockExperimentalVMOptions", "-XX:+DisableExplicitGC",
    "-XX:G1NewSizePercent=30", "-XX:G1MaxNewSizePercent=40", "-XX:G1HeapRegionSize=8M",
    "-XX:G1ReservePercent=20", "-XX:G1HeapWastePercent=5", "-XX:G1MixedGCCountTarget=4",
    "-XX:InitiatingHeapOccupancyPercent=15", "-XX:G1MixedGCLiveThresholdPercent=90",
    "-XX:G1RSetUpdatingPauseTimePercent=5", "-XX:SurvivorRatio=32",
    "-XX:+PerfDisableSharedMem", "-XX:MaxTenuringThreshold=1",
];

const FPS_OPTIONS: &str = "enableVsync:false\nmaxFps:260\nrenderDistance:10\nsimulationDistance:8\ngraphicsMode:0\nparticles:1\nentityDistanceScaling:0.75\n";

// ---------------------------------------------------------------- config
#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
struct Config {
    premium: bool,
    nick: String,
    ram: u32,
    client_id: String,
    refresh_token: String,
    ms_name: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            premium: false,
            nick: "Player".into(),
            ram: 4,
            client_id: String::new(),
            refresh_token: String::new(),
            ms_name: String::new(),
        }
    }
}

fn root() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".chaoticclient")
}

#[derive(Default)]
struct Shared {
    status: String,
    progress: f32,
    busy: bool,
    device_msg: String,
    pending: Option<(String, String)>, // (refresh_token, name)
}

fn set(sh: &Arc<Mutex<Shared>>, s: &str, p: f32) {
    let mut g = sh.lock().unwrap();
    g.status = s.to_string();
    if p >= 0.0 {
        g.progress = p;
    }
}

// ---------------------------------------------------------------- helpers
#[cfg(windows)]
fn hide(c: &mut Command) {
    use std::os::windows::process::CommandExt;
    c.creation_flags(0x0800_0000);
}
#[cfg(not(windows))]
fn hide(_c: &mut Command) {}

fn open_url(url: &str) {
    let mut c = Command::new("cmd");
    c.args(["/C", "start", "", url]);
    hide(&mut c);
    let _ = c.spawn();
}

fn http() -> Client {
    Client::builder()
        .user_agent("ChaoticClient/1.0")
        .timeout(Duration::from_secs(300))
        .build()
        .unwrap()
}

fn read_json(p: &Path) -> Option<Value> {
    serde_json::from_str(&fs::read_to_string(p).ok()?).ok()
}

fn get_json(c: &Client, url: &str) -> Res<Value> {
    Ok(c.get(url).send()?.error_for_status()?.json::<Value>()?)
}

fn cached_json(c: &Client, url: &str, path: &Path) -> Res<Value> {
    if let Some(v) = read_json(path) {
        return Ok(v);
    }
    let v = get_json(c, url)?;
    if let Some(p) = path.parent() {
        fs::create_dir_all(p)?;
    }
    fs::write(path, serde_json::to_string(&v)?)?;
    Ok(v)
}

fn download(c: &Client, url: &str, path: &Path) -> Res<()> {
    if path.exists() {
        return Ok(());
    }
    if let Some(p) = path.parent() {
        fs::create_dir_all(p)?;
    }
    let bytes = c.get(url).send()?.error_for_status()?.bytes()?;
    let tmp = path.with_extension("part");
    fs::write(&tmp, &bytes)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

fn download_retry(c: &Client, url: &str, path: &Path) -> Res<()> {
    let mut last: Res<()> = Ok(());
    for _ in 0..3 {
        last = download(c, url, path);
        if last.is_ok() {
            break;
        }
        thread::sleep(Duration::from_millis(400));
    }
    last
}

fn download_many(
    c: &Client,
    jobs: Vec<(String, PathBuf)>,
    sh: &Arc<Mutex<Shared>>,
    label: &str,
    p0: f32,
    p1: f32,
) -> Res<()> {
    let jobs: Vec<(String, PathBuf)> = jobs.into_iter().filter(|(_, p)| !p.exists()).collect();
    let total = jobs.len();
    if total == 0 {
        return Ok(());
    }
    let queue = Arc::new(Mutex::new(jobs));
    let done = Arc::new(AtomicUsize::new(0));
    let failed = Arc::new(Mutex::new(None::<String>));
    let mut hs = Vec::new();
    for _ in 0..16usize.min(total) {
        let (q, d, f, c, sh) = (queue.clone(), done.clone(), failed.clone(), c.clone(), sh.clone());
        let label = label.to_string();
        hs.push(thread::spawn(move || loop {
            if f.lock().unwrap().is_some() {
                break;
            }
            let job = q.lock().unwrap().pop();
            let Some((url, path)) = job else { break };
            if let Err(e) = download_retry(&c, &url, &path) {
                *f.lock().unwrap() = Some(format!("{url}: {e}"));
                break;
            }
            let n = d.fetch_add(1, Ordering::SeqCst) + 1;
            if n % 10 == 0 || n == total {
                set(&sh, &format!("{label} {n}/{total}"), p0 + (p1 - p0) * (n as f32 / total as f32));
            }
        }));
    }
    for h in hs {
        let _ = h.join();
    }
    let err = failed.lock().unwrap().take();
    if let Some(e) = err {
        return Err(e.into());
    }
    Ok(())
}

fn unzip(zip_path: &Path, dest: &Path) -> Res<()> {
    let mut ar = zip::ZipArchive::new(fs::File::open(zip_path)?)?;
    for i in 0..ar.len() {
        let mut f = ar.by_index(i)?;
        let Some(rel) = f.enclosed_name() else { continue };
        let out = dest.join(rel);
        if f.is_dir() {
            fs::create_dir_all(&out)?;
        } else {
            if let Some(p) = out.parent() {
                fs::create_dir_all(p)?;
            }
            let mut o = fs::File::create(&out)?;
            std::io::copy(&mut f, &mut o)?;
        }
    }
    Ok(())
}

fn find_java(dir: &Path) -> Option<PathBuf> {
    for e in fs::read_dir(dir).ok()?.flatten() {
        let p = e.path().join("bin").join("java.exe");
        if p.exists() {
            return Some(p);
        }
    }
    None
}

fn offline_uuid(name: &str) -> String {
    let mut h = md5::compute(format!("OfflinePlayer:{name}")).0;
    h[6] = (h[6] & 0x0f) | 0x30;
    h[8] = (h[8] & 0x3f) | 0x80;
    let s: String = h.iter().map(|b| format!("{b:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &s[0..8], &s[8..12], &s[12..16], &s[16..20], &s[20..])
}

// ---------------------------------------------------------------- libraries
struct Lib {
    key: String,
    path: PathBuf,
    url: String,
}

fn rules_ok(lib: &Value) -> bool {
    let Some(rules) = lib.get("rules").and_then(|r| r.as_array()) else {
        return true;
    };
    let mut allowed = false;
    for r in rules {
        let allow = r["action"].as_str() == Some("allow");
        let matches = match r.get("os") {
            None => true,
            Some(os) => os["name"].as_str().map_or(true, |n| n == "windows"),
        };
        if matches {
            allowed = allow;
        }
    }
    allowed
}

fn collect_libs(v: &Value, dir: &Path) -> Vec<Lib> {
    let mut out = Vec::new();
    let Some(arr) = v["libraries"].as_array() else {
        return out;
    };
    for l in arr {
        if !rules_ok(l) {
            continue;
        }
        let Some(name) = l["name"].as_str() else { continue };
        if name.contains("natives-linux")
            || name.contains("natives-macos")
            || name.contains("natives-windows-arm64")
            || name.ends_with("natives-windows-x86")
        {
            continue;
        }
        let parts: Vec<&str> = name.split(':').collect();
        if parts.len() < 3 {
            continue;
        }
        let key = if parts.len() > 3 {
            format!("{}:{}:{}", parts[0], parts[1], parts[3])
        } else {
            format!("{}:{}", parts[0], parts[1])
        };
        let art = &l["downloads"]["artifact"];
        let (rel, url) = if let (Some(p), Some(u)) = (art["path"].as_str(), art["url"].as_str()) {
            (p.to_string(), u.to_string())
        } else if let Some(base) = l["url"].as_str() {
            let mut rel = format!(
                "{}/{}/{}/{}-{}",
                parts[0].replace('.', "/"), parts[1], parts[2], parts[1], parts[2]
            );
            if parts.len() > 3 {
                rel.push_str(&format!("-{}", parts[3]));
            }
            rel.push_str(".jar");
            let sep = if base.ends_with('/') { "" } else { "/" };
            let u = format!("{base}{sep}{rel}");
            (rel, u)
        } else {
            continue;
        };
        out.push(Lib { key, path: dir.join(rel), url });
    }
    out
}

// ---------------------------------------------------------------- mods
fn install_mods(c: &Client, mods: &Path, sh: &Arc<Mutex<Shared>>) -> Res<()> {
    fs::create_dir_all(mods)?;
    for (i, slug) in PERF_MODS.iter().enumerate() {
        set(sh, &format!("Modovi za FPS: {slug}"), 0.80 + 0.15 * (i as f32 / PERF_MODS.len() as f32));
        let r = c
            .get(format!("https://api.modrinth.com/v2/project/{slug}/version"))
            .query(&[
                ("game_versions", format!("[\"{MC}\"]")),
                ("loaders", "[\"fabric\"]".to_string()),
            ])
            .send();
        let Ok(resp) = r else { continue };
        let Ok(v) = resp.json::<Value>() else { continue };
        let Some(ver) = v.get(0) else { continue };
        let Some(files) = ver["files"].as_array() else { continue };
        let Some(f) = files.iter().find(|f| f["primary"].as_bool() == Some(true)).or(files.first()) else {
            continue;
        };
        let (Some(url), Some(name)) = (f["url"].as_str(), f["filename"].as_str()) else { continue };
        let _ = download_retry(c, url, &mods.join(name));
    }
    Ok(())
}

// ---------------------------------------------------------------- premium auth
struct Session {
    token: String,
    name: String,
    uuid: String,
}

fn ms_token(c: &Client, cfg: &Config, sh: &Arc<Mutex<Shared>>) -> Res<(String, String)> {
    let cid = cfg.client_id.trim();
    if cid.is_empty() {
        return Err("Upiši Azure Client ID (vidi README).".into());
    }
    if !cfg.refresh_token.is_empty() {
        let r = c
            .post(TOKEN_URL)
            .form(&[
                ("client_id", cid),
                ("grant_type", "refresh_token"),
                ("refresh_token", cfg.refresh_token.as_str()),
                ("scope", SCOPE),
            ])
            .send()?;
        if r.status().is_success() {
            let v: Value = r.json()?;
            if let Some(a) = v["access_token"].as_str() {
                let rt = v["refresh_token"].as_str().unwrap_or(&cfg.refresh_token).to_string();
                return Ok((a.to_string(), rt));
            }
        }
    }
    let d: Value = c
        .post(DEVICE_URL)
        .form(&[("client_id", cid), ("scope", SCOPE)])
        .send()?
        .error_for_status()?
        .json()?;
    let code = d["user_code"].as_str().ok_or("Greška kod prijave")?.to_string();
    let uri = d["verification_uri"].as_str().ok_or("Greška kod prijave")?.to_string();
    let dc = d["device_code"].as_str().ok_or("Greška kod prijave")?.to_string();
    let interval = d["interval"].as_u64().unwrap_or(5).max(1);
    sh.lock().unwrap().device_msg = format!("Otvori {uri} i upiši kod: {code}");
    open_url(&uri);
    for _ in 0..(900 / interval) {
        thread::sleep(Duration::from_secs(interval));
        let r = c
            .post(TOKEN_URL)
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("client_id", cid),
                ("device_code", dc.as_str()),
            ])
            .send()?;
        let v: Value = r.json()?;
        if let Some(a) = v["access_token"].as_str() {
            sh.lock().unwrap().device_msg.clear();
            return Ok((a.to_string(), v["refresh_token"].as_str().unwrap_or("").to_string()));
        }
        match v["error"].as_str() {
            Some("authorization_pending") | Some("slow_down") => continue,
            Some(e) => {
                sh.lock().unwrap().device_msg.clear();
                return Err(format!("Prijava neuspješna: {e}").into());
            }
            None => {}
        }
    }
    sh.lock().unwrap().device_msg.clear();
    Err("Isteklo je vrijeme za prijavu.".into())
}

fn premium_session(c: &Client, cfg: &Config, sh: &Arc<Mutex<Shared>>) -> Res<Session> {
    set(sh, "Prijava u Microsoft...", -1.0);
    let (ms_access, refresh) = ms_token(c, cfg, sh)?;
    let xbl: Value = c
        .post("https://user.auth.xboxlive.com/user/authenticate")
        .json(&json!({
            "Properties": {"AuthMethod": "RPS", "SiteName": "user.auth.xboxlive.com",
                           "RpsTicket": format!("d={ms_access}")},
            "RelyingParty": "http://auth.xboxlive.com", "TokenType": "JWT"
        }))
        .send()?
        .error_for_status()?
        .json()?;
    let xt = xbl["Token"].as_str().ok_or("Xbox prijava nije uspjela")?.to_string();
    let uhs = xbl["DisplayClaims"]["xui"][0]["uhs"].as_str().ok_or("Xbox prijava nije uspjela")?.to_string();
    let xsts: Value = c
        .post("https://xsts.auth.xboxlive.com/xsts/authorize")
        .json(&json!({
            "Properties": {"SandboxId": "RETAIL", "UserTokens": [xt]},
            "RelyingParty": "rp://api.minecraftservices.com/", "TokenType": "JWT"
        }))
        .send()?
        .error_for_status()?
        .json()?;
    let xs = xsts["Token"].as_str().ok_or("XSTS prijava nije uspjela")?.to_string();
    let r = c
        .post("https://api.minecraftservices.com/authentication/login_with_xbox")
        .json(&json!({"identityToken": format!("XBL3.0 x={uhs};{xs}")}))
        .send()?;
    if r.status().as_u16() == 403 {
        return Err("Mojang je odbio Azure aplikaciju (treba odobrenje: aka.ms/AppRegInfo).".into());
    }
    let mc: Value = r.error_for_status()?.json()?;
    let token = mc["access_token"].as_str().ok_or("Minecraft prijava nije uspjela")?.to_string();
    let r = c.get("https://api.minecraftservices.com/minecraft/profile").bearer_auth(&token).send()?;
    if r.status().as_u16() == 404 {
        return Err("Ovaj račun nema Minecraft Java Edition.".into());
    }
    let p: Value = r.error_for_status()?.json()?;
    let name = p["name"].as_str().ok_or("Profil nije pronađen")?.to_string();
    let uuid = p["id"].as_str().ok_or("Profil nije pronađen")?.to_string();
    sh.lock().unwrap().pending = Some((refresh, name.clone()));
    Ok(Session { token, name, uuid })
}

fn change_skin(sh: &Arc<Mutex<Shared>>, cfg: &Config, path: PathBuf, slim: bool) -> Res<()> {
    let c = http();
    let s = premium_session(&c, cfg, sh)?;
    set(sh, "Šaljem skin...", -1.0);
    let form = reqwest::blocking::multipart::Form::new()
        .text("variant", if slim { "slim" } else { "classic" })
        .file("file", &path)?;
    let r = c
        .post("https://api.minecraftservices.com/minecraft/profile/skins")
        .bearer_auth(&s.token)
        .multipart(form)
        .send()?;
    if !r.status().is_success() {
        let t = r.text().unwrap_or_default();
        return Err(format!("Skin nije promijenjen: {t}").into());
    }
    set(sh, "Skin promijenjen! Vidi ga pri sljedećem ulasku u igru.", -1.0);
    Ok(())
}

// ---------------------------------------------------------------- launch
fn launch(sh: &Arc<Mutex<Shared>>, cfg: &Config) -> Res<()> {
    let c = http();
    let root = root();
    let game = root.join("minecraft");
    let libs_dir = game.join("libraries");
    let assets = game.join("assets");
    let mods = game.join("mods");
    fs::create_dir_all(&mods)?;

    // racun
    let (name, uuid, token, utype) = if cfg.premium {
        let s = premium_session(&c, cfg, sh)?;
        (s.name, s.uuid, s.token, "msa")
    } else {
        (cfg.nick.clone(), offline_uuid(&cfg.nick), "0".to_string(), "legacy")
    };

    // java
    set(sh, "Provjeravam Javu...", 0.01);
    let jre_dir = root.join("runtime");
    let java = match find_java(&jre_dir) {
        Some(j) => j,
        None => {
            set(sh, "Skidam Javu 21 (samo prvi put)...", 0.02);
            let zip = jre_dir.join("jre21.zip");
            download(
                &c,
                "https://api.adoptium.net/v3/binary/latest/21/ga/windows/x64/jre/hotspot/normal/eclipse",
                &zip,
            )?;
            set(sh, "Raspakiravam Javu...", 0.08);
            unzip(&zip, &jre_dir)?;
            let _ = fs::remove_file(&zip);
            find_java(&jre_dir).ok_or("Java nije pronađena nakon raspakiravanja")?
        }
    };

    // verzije
    set(sh, "Provjeravam verziju igre...", 0.10);
    let vpath = game.join("versions").join(MC).join(format!("{MC}.json"));
    let vanilla = match read_json(&vpath) {
        Some(v) => v,
        None => {
            let m = get_json(&c, "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json")?;
            let url = m["versions"]
                .as_array()
                .and_then(|a| a.iter().find(|v| v["id"] == MC))
                .and_then(|v| v["url"].as_str())
                .ok_or("Verzija 1.21.11 nije pronađena")?
                .to_string();
            cached_json(&c, &url, &vpath)?
        }
    };
    let fpath = game.join("versions").join(format!("fabric-{MC}.json"));
    let fabric = match read_json(&fpath) {
        Some(v) => v,
        None => {
            let l = get_json(&c, &format!("https://meta.fabricmc.net/v2/versions/loader/{MC}"))?;
            let lv = l[0]["loader"]["version"]
                .as_str()
                .ok_or("Fabric još ne podržava 1.21.11")?
                .to_string();
            cached_json(
                &c,
                &format!("https://meta.fabricmc.net/v2/versions/loader/{MC}/{lv}/profile/json"),
                &fpath,
            )?
        }
    };

    // biblioteke
    let mut seen = HashSet::new();
    let mut libs: Vec<Lib> = Vec::new();
    for l in collect_libs(&fabric, &libs_dir).into_iter().chain(collect_libs(&vanilla, &libs_dir)) {
        if seen.insert(l.key.clone()) {
            libs.push(l);
        }
    }
    let jobs: Vec<(String, PathBuf)> = libs.iter().map(|l| (l.url.clone(), l.path.clone())).collect();
    download_many(&c, jobs, sh, "Biblioteke", 0.12, 0.25)?;

    // client jar
    set(sh, "Provjeravam igru...", 0.26);
    let client_jar = game.join("versions").join(MC).join(format!("{MC}.jar"));
    let curl = vanilla["downloads"]["client"]["url"].as_str().ok_or("Nema client jara")?;
    download_retry(&c, curl, &client_jar)?;

    // assets
    let ai_id = vanilla["assetIndex"]["id"].as_str().ok_or("Nema asset indexa")?.to_string();
    let ai_url = vanilla["assetIndex"]["url"].as_str().ok_or("Nema asset indexa")?.to_string();
    let idx = cached_json(&c, &ai_url, &assets.join("indexes").join(format!("{ai_id}.json")))?;
    let mut ajobs = Vec::new();
    if let Some(objs) = idx["objects"].as_object() {
        for (_, o) in objs {
            if let Some(h) = o["hash"].as_str() {
                let p = &h[..2];
                ajobs.push((
                    format!("https://resources.download.minecraft.net/{p}/{h}"),
                    assets.join("objects").join(p).join(h),
                ));
            }
        }
    }
    download_many(&c, ajobs, sh, "Resursi igre", 0.28, 0.78)?;

    // modovi (jednokratno)
    let marker = root.join(format!("mods_{MC}.ok"));
    if !marker.exists() {
        install_mods(&c, &mods, sh)?;
        let _ = fs::write(&marker, "ok");
    }

    let opts = game.join("options.txt");
    if !opts.exists() {
        let _ = fs::write(&opts, FPS_OPTIONS);
    }

    // pokretanje
    set(sh, "Pokrećem igru...", 0.97);
    let mut cp: Vec<String> = libs.iter().map(|l| l.path.to_string_lossy().to_string()).collect();
    cp.push(client_jar.to_string_lossy().to_string());
    let main_class = fabric["mainClass"].as_str().unwrap_or("net.fabricmc.loader.impl.launch.knot.KnotClient");

    let ram = cfg.ram.clamp(2, 32);
    let mut args: Vec<String> = vec![format!("-Xms{}G", ram.min(2)), format!("-Xmx{ram}G")];
    args.extend(JVM_FLAGS.iter().map(|s| s.to_string()));
    args.push("-Dminecraft.launcher.brand=ChaoticClient".into());
    args.push("-Dminecraft.launcher.version=1.0".into());
    if let Some(a) = fabric["arguments"]["jvm"].as_array() {
        for x in a {
            if let Some(s) = x.as_str() {
                if !s.contains("${") {
                    args.push(s.to_string());
                }
            }
        }
    }
    args.push("-cp".into());
    args.push(cp.join(";"));
    args.push(main_class.to_string());
    for (k, v) in [
        ("--username", name.as_str()),
        ("--version", MC),
        ("--gameDir", &game.to_string_lossy()),
        ("--assetsDir", &assets.to_string_lossy()),
        ("--assetIndex", ai_id.as_str()),
        ("--uuid", uuid.as_str()),
        ("--accessToken", token.as_str()),
        ("--userType", utype),
        ("--versionType", "ChaoticClient"),
    ] {
        args.push(k.to_string());
        args.push(v.to_string());
    }

    let log_path = root.join("launch.log");
    let log = fs::File::create(&log_path)?;
    let log2 = log.try_clone()?;
    let mut cmd = Command::new(&java);
    cmd.args(&args).current_dir(&game).stdout(Stdio::from(log)).stderr(Stdio::from(log2));
    hide(&mut cmd);
    let mut child = cmd.spawn()?;
    for _ in 0..50 {
        thread::sleep(Duration::from_millis(500));
        if let Some(st) = child.try_wait()? {
            return Err(format!("Igra se ugasila ({st}). Pošalji mi datoteku: {}", log_path.display()).into());
        }
    }
    set(sh, "Igra pokrenuta. Uživaj!", 1.0);
    Ok(())
}

// ---------------------------------------------------------------- UI
struct App {
    cfg: Config,
    last_saved: String,
    sh: Arc<Mutex<Shared>>,
    slim: bool,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut v = egui::Visuals::dark();
        v.panel_fill = BG;
        v.extreme_bg_color = Color32::from_rgb(18, 14, 36);
        v.selection.bg_fill = ACCENT;
        v.widgets.inactive.weak_bg_fill = Color32::from_rgb(42, 34, 80);
        v.widgets.hovered.weak_bg_fill = Color32::from_rgb(59, 47, 110);
        cc.egui_ctx.set_visuals(v);
        let cfg: Config = read_json(&root().join("config.json"))
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();
        let sh = Arc::new(Mutex::new(Shared { status: "Spreman.".into(), ..Default::default() }));
        Self { cfg, last_saved: String::new(), sh, slim: false }
    }

    fn save(&mut self) {
        if let Ok(s) = serde_json::to_string_pretty(&self.cfg) {
            if s != self.last_saved {
                let _ = fs::create_dir_all(root());
                let _ = fs::write(root().join("config.json"), &s);
                self.last_saved = s;
            }
        }
    }

    fn start(&mut self) {
        let nick: String = self
            .cfg
            .nick
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
            .take(16)
            .collect();
        if !self.cfg.premium && nick.len() < 3 {
            set(&self.sh, "Nick mora imati 3-16 znakova (slova, brojevi, _).", -1.0);
            return;
        }
        self.cfg.nick = nick;
        self.save();
        {
            let mut s = self.sh.lock().unwrap();
            s.busy = true;
            s.progress = 0.0;
        }
        let (sh, cfg) = (self.sh.clone(), self.cfg.clone());
        thread::spawn(move || {
            let r = launch(&sh, &cfg);
            let mut s = sh.lock().unwrap();
            if let Err(e) = r {
                s.status = format!("Greška: {e}");
            }
            s.busy = false;
        });
    }

    fn start_skin(&mut self, path: PathBuf) {
        self.save();
        self.sh.lock().unwrap().busy = true;
        let (sh, cfg, slim) = (self.sh.clone(), self.cfg.clone(), self.slim);
        thread::spawn(move || {
            let r = change_skin(&sh, &cfg, path, slim);
            let mut s = sh.lock().unwrap();
            if let Err(e) = r {
                s.status = format!("Greška: {e}");
            }
            s.busy = false;
        });
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _f: &mut eframe::Frame) {
        let (status, progress, busy, device, pending) = {
            let mut s = self.sh.lock().unwrap();
            (s.status.clone(), s.progress, s.busy, s.device_msg.clone(), s.pending.take())
        };
        if let Some((rt, name)) = pending {
            if !rt.is_empty() {
                self.cfg.refresh_token = rt;
            }
            self.cfg.ms_name = name;
        }
        if busy {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(BG).inner_margin(egui::Margin::same(24.0)))
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new("CHAOTIC").size(42.0).strong().color(ACCENT));
                    ui.label(RichText::new(format!("CLIENT  -  Minecraft {MC}")).color(MUTED));
                });
                ui.add_space(14.0);

                egui::Frame::none().fill(CARD).rounding(14.0).inner_margin(egui::Margin::same(16.0)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut self.cfg.premium, false, RichText::new("  Cracked  ").size(16.0));
                        ui.selectable_value(&mut self.cfg.premium, true, RichText::new("  Premium  ").size(16.0));
                    });
                    ui.add_space(8.0);
                    if self.cfg.premium {
                        let who = if self.cfg.ms_name.is_empty() { "Nisi prijavljen (prijava ide pri PLAY)".to_string() } else { format!("Račun: {}", self.cfg.ms_name) };
                        ui.label(RichText::new(who).color(MUTED));
                        ui.label(RichText::new("Azure Client ID").color(MUTED).size(12.0));
                        ui.add(egui::TextEdit::singleline(&mut self.cfg.client_id).desired_width(f32::INFINITY));
                    } else {
                        ui.label(RichText::new("Nick").color(MUTED).size(12.0));
                        ui.add(egui::TextEdit::singleline(&mut self.cfg.nick).desired_width(f32::INFINITY));
                    }
                    ui.add_space(8.0);
                    ui.label(RichText::new("RAM").color(MUTED).size(12.0));
                    ui.add(egui::Slider::new(&mut self.cfg.ram, 2..=16).suffix(" GB"));
                });

                ui.add_space(12.0);
                let w = (ui.available_width() - 8.0) / 2.0;
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new("Dodaj mod").fill(CARD).rounding(8.0).min_size(egui::vec2(w, 34.0))).clicked() {
                        if let Some(files) = rfd::FileDialog::new().add_filter("Mod (.jar)", &["jar"]).pick_files() {
                            let dir = root().join("minecraft").join("mods");
                            let _ = fs::create_dir_all(&dir);
                            for f in files {
                                if let Some(n) = f.file_name() {
                                    let _ = fs::copy(&f, dir.join(n));
                                }
                            }
                            set(&self.sh, "Mod(ovi) dodani.", -1.0);
                        }
                    }
                    if ui.add(egui::Button::new("Mods folder").fill(CARD).rounding(8.0).min_size(egui::vec2(w, 34.0))).clicked() {
                        let dir = root().join("minecraft").join("mods");
                        let _ = fs::create_dir_all(&dir);
                        let _ = Command::new("explorer").arg(dir).spawn();
                    }
                });

                ui.add_space(8.0);
                if self.cfg.premium {
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut self.slim, "Slim (Alex)");
                        if ui.add_enabled(!busy, egui::Button::new("Promijeni skin (.png)").fill(CARD).rounding(8.0)).clicked() {
                            if let Some(p) = rfd::FileDialog::new().add_filter("Skin PNG", &["png"]).pick_file() {
                                self.start_skin(p);
                            }
                        }
                    });
                } else {
                    ui.label(RichText::new("Promjena skina radi samo s Premium računom.").color(MUTED).size(12.0));
                }

                ui.add_space(12.0);
                let play = egui::Button::new(
                    RichText::new(if busy { "RADIM..." } else { "PLAY" }).size(26.0).strong().color(Color32::WHITE),
                )
                .fill(ACCENT)
                .rounding(14.0)
                .min_size(egui::vec2(ui.available_width(), 58.0));
                if ui.add_enabled(!busy, play).clicked() {
                    self.start();
                }
                ui.add_space(12.0);
                ui.add(egui::ProgressBar::new(progress).fill(ACCENT).desired_height(8.0));
                ui.add_space(6.0);
                if !device.is_empty() {
                    ui.label(RichText::new(device).strong().color(Color32::WHITE));
                }
                ui.label(RichText::new(status).color(MUTED));
            });

        self.save();
    }
}

fn main() -> eframe::Result<()> {
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([520.0, 690.0])
            .with_resizable(false)
            .with_title("ChaoticClient"),
        ..Default::default()
    };
    eframe::run_native("ChaoticClient", opts, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}
