# ChaoticClient (Rust) - Minecraft 1.21.11

Brzi nativni launcher: jedan .exe (~8 MB), nema Pythona ni instalacije.

## Kako dobiti ChaoticClient.exe (bez instalacije Rusta)
1. Napravi besplatan racun na github.com i novi repozitorij ("New repository").
2. Uploadaj SAV sadrzaj ovog foldera (Cargo.toml, src, .github ...) -> "uploading an existing file".
3. Otvori tab **Actions** -> pricekaj ~5 min da build postane zelen.
4. Klikni zadnji build -> dolje **Artifacts -> ChaoticClient** -> skini zip -> unutra je .exe.

(Alternativa: instaliraj Rust s rustup.rs i dvaput klikni BUILD.bat.)

## Upotreba
- **Cracked:** nick + PLAY (samo offline-mode serveri / singleplayer).
- **Premium:** treba Azure Client ID ODOBREN od Mojanga (aka.ms/AppRegInfo) - bez toga Mojang odbija prijavu.
  Prijava ide kodom: launcher otvori microsoft.com/link, upisi kod koji pise u launcheru.
- **Skin:** samo Premium. Odaberes .png, Classic/Slim, gotovo.
- **Performanse:** Fabric + Sodium, Lithium, FerriteCore, ImmediatelyFast, EntityCulling, ModernFix, Krypton, Sodium Extra,
  JVM flagovi, vsync off, bez FPS limita. Svako sljedece pokretanje ne koristi internet.
- Podaci: %USERPROFILE%\.chaoticclient (ako igra ne krene, vidi launch.log).
