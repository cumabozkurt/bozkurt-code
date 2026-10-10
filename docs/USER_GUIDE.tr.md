# BOZKURT CODE: Kullanım Kılavuzu

*[English](USER_GUIDE.md)*

Bu kılavuz sizi kurulumdan çalışan bir `https://uygulamam.test` adresine kadar götürür; ardından masaüstü uygulamasında ve
komut satırında günlük işleri anlatır. Komutların tam sözdizimi için [CLI.md](CLI.md), yapay zekâ ajanları için
[MCP.md](MCP.md) dosyasına bakın.

Bu kılavuz 1.0 sürümünü anlatır. Platform sınırları (ör. paketle gelen MariaDB'nin hangi sistemlerde olduğu)
[README](../README.tr.md) ve [yol haritası](ROADMAP.md#known-limitations-in-10) sayfalarında yazar.

## İçindekiler

1. [Temel kavramlar](#bolum-1)
2. [Kurulum](#bolum-2)
3. [İlk açılış: Hızlı kurulum](#bolum-3)
4. [Siteler](#bolum-4)
5. [Çalışma ortamları (PHP, Node.js, Python, Bun, Go)](#bolum-5)
6. [Servisler ve veritabanları](#bolum-6)
7. [HTTPS ve yerel sertifika otoritesi](#bolum-7)
8. [Yerel DNS](#bolum-8)
9. [Günlükler, işler ve Doktor](#bolum-9)
10. [Bir siteyi herkese açmak](#bolum-10)
11. [Mobil eşlikçi](#bolum-11)
12. [Yapay zekâ ajanları (MCP)](#bolum-12)
13. [MAMP, Valet, Herd, XAMPP ve Laragon'dan geçiş](#bolum-13)
14. [Ayarlar ve dosyalar](#bolum-14)
15. [Kaldırma](#bolum-15)
16. [Sorun giderme](#bolum-16)

<a id="bolum-1"></a>

## 1. Temel kavramlar

- **Ana klasör.** BOZKURT'un kurduğu her şey tek bir klasörde durur:
  - macOS/Linux: `~/.bozkurt`
  - Windows: `%LOCALAPPDATA%\Bozkurt`
  - Başka bir klasör kullanmak için `BOZKURT_HOME` değişkenini ayarlayın.
- **Arka plan hizmeti (daemon).** `bozkurtd`, arka planda çalışan küçük bir süreçtir. Servisleri o yönetir ve
  `127.0.0.1:7878` adresinde yerel bir API sunar. Masaüstü uygulaması, `bz`, `bozkurt-mcp` ve telefon uygulaması onun
  istemcileridir; hangisini kullandığınız fark etmez. Web sayfaları bu API'yi kullanamaz: `bz` ve `bozkurt-mcp`,
  arka plan hizmetinin `$BOZKURT_HOME/api-token` dosyasında tuttuğu anahtarla kimlik doğrular ve bu dosyayı kendiliğinden
  okur (ayrıntılar: [SECURITY.md](../SECURITY.md#local-api-protection)).
- **Site.** `magaza.test` gibi bir alan adına bağlanmış proje klasörü. Dört türü vardır:

  | Tür | Ne olur |
  |---|---|
  | `php` | Caddy dosyaları sunar, `.php` isteklerini sitenin PHP sürümüne ait PHP-FPM havuzuna gönderir (Windows'ta php-cgi). |
  | `app` | BOZKURT geliştirme komutunuzu (ör. `npm run dev`) çalıştırır, `PORT` değişkenini atar; Caddy istekleri ona iletir. |
  | `proxy` | Caddy alan adını, zaten bir şeyin dinlediği bir porta yönlendirir. |
  | `static` | Caddy bir klasördeki dosyaları olduğu gibi sunar (ör. `dist/`). |

- **Çalışma ortamı (runtime).** PHP, Node.js, Python, Bun ya da Go'nun kurulu bir sürümü. Her birinin bir sürümü
  *genel* (global) sürümdür: `current/` altında bağlanır, komutları `bin/` altındadır. Her site kendi sürümünü sabitleyebilir.
- **Servis.** BOZKURT'un sürekli gözettiği uzun ömürlü süreçler: `caddy`, `dns`, `mariadb`, `postgres`, `redis`,
  `mailpit` ve `php-fpm-X.Y` havuzları.
- **İş (job).** İndirme ya da DNS kurulumu gibi uzun süren her işlem, ilerleme çubuğu olan bir iş olarak çalışır.
  Pencereyi kapatsanız da iş devam eder.

<a id="bolum-2"></a>

## 2. Kurulum

**Masaüstü uygulaması:** İşletim sisteminize uygun kurulum dosyasını GitHub Releases sayfasından indirip açın (`.dmg`,
`.msi` / `-setup.exe`, `.deb` / `.rpm` / `.AppImage`). Uygulama `bozkurtd`, `bz` ve `bozkurt-mcp` dosyalarını kendi paketinin
içinde taşır. `bz`'yi terminalden kullanmak ya da `bozkurt-mcp`'yi bir editöre tanıtmak için aşağıdaki CLI kurulumunu da yapın.

**Yalnızca CLI:**

```sh
# macOS / Linux
curl -fsSL https://raw.githubusercontent.com/cumabozkurt/bozkurt-code/main/scripts/install.sh | sh
```
```powershell
# Windows
irm https://raw.githubusercontent.com/cumabozkurt/bozkurt-code/main/scripts/install.ps1 | iex
```

Betikler her indirmeyi sürümdeki `SHA256SUMS` dosyasıyla doğrular ve `$BOZKURT_HOME/bin` klasörüne kurar. Root yetkisi
hiçbir zaman gerekmez. Bu klasör `PATH` içinde değilse eklemeniz gereken satırı tam olarak yazarlar (Windows'ta betiği
`-AddToPath` ile yeniden çalıştırabilirsiniz).

**Kabuk tamamlama:** `bz completions zsh|bash|fish|powershell`.

<a id="bolum-3"></a>

## 3. İlk açılış: Hızlı kurulum

Uygulamayı açtığınızda **Genel bakış** ekranı neyin hazır, neyin eksik olduğunu gösterir. **Hızlı kurulumu başlat** düğmesi sizi şu adımlardan geçirir:

1. **Portlar.** 80 ve 443 numaralı portları denetler. Bunları başka bir program (Apache, nginx, başka bir geliştirme
   aracı) tutuyorsa BOZKURT hangi süreç olduğunu gösterir. Kararı siz verirsiniz: o programı kendiniz durdurun ya da
   8080/8443'ü kullanın.
2. **DNS.** `*.test` alan adlarını BOZKURT'a yönlendirir. Bir şey değişmeden önce hangi dosyanın ya da kuralın yazılacağını
   görürsünüz; ardından işletim sistemi kendi yönetici onayını sorar. Önceki durum yedeklenir.
3. **HTTPS.** BOZKURT'un yerel sertifika otoritesini (CA) *kendi kullanıcı* sertifika deponuza güvenilir olarak ekler.
   macOS ve Windows'ta bu genellikle yönetici yetkisi gerektirmez.
4. **Çalışma ortamları.** Seçtiğiniz PHP ve Node.js sürümlerini kurar.
5. **Servisler.** İsterseniz MariaDB, PostgreSQL, Redis ve Mailpit'i kurar ve otomatik başlayacak şekilde ayarlar.

Aynı adımlar terminalde şöyledir:

```sh
bz doctor
bz dns setup
bz trust
bz php install 8.3 && bz node install lts
bz start all
```

<a id="bolum-4"></a>

## 4. Siteler

### Site oluşturma

**Uygulamada:** **Siteler → Yeni site** deyip klasörü seçin. BOZKURT çatıyı (framework) tanır; alan adı, tür, belge kökü
ve sürümleri sizin için doldurur. İstediğinizi değiştirin, HTTPS'i açın ve **Oluştur**'a basın.

**CLI ile:**

```sh
cd ~/kod/magaza
bz link --https                 # → https://magaza.test
bz link --domain dukkan         # → dukkan.test
bz link ./frontend --domain app --command "npm run dev" --https
bz link --domain api --proxy 8000          # zaten :8000'de dinleyen bir şey varsa
```

Otomatik tanınanlar:

| Çatı | Tür | Kök | Komut |
|---|---|---|---|
| Laravel, Symfony | php | `public` | — |
| WordPress | php | proje kökü (Bedrock için `web`) | — |
| Drupal | php | `web` ya da kök | — |
| Next.js, Nuxt, Astro, Remix / React Router | app | — | `--port $PORT` ile dev betiği (ör. `npm run dev -- --port $PORT`) |
| Vite, SvelteKit | app | — | `--port $PORT --strictPort` ile dev betiği |
| Express / Fastify / Koa | app | — | `dev` betiği (yoksa `start`) |
| Django | app | — | `python manage.py runserver 127.0.0.1:$PORT` |
| FastAPI | app | — | `python -m uvicorn main:app --host 127.0.0.1 --port $PORT --reload` |
| Flask | app | — | `python -m flask run --host 127.0.0.1 --port $PORT --debug` |
| Go (`go.mod`) | app | — | `go run .` |
| `index.php` / `composer.json` | php | `public/index.php` varsa `public` | — |
| Diğer her şey | static | `index.html` içeriyorsa `public`, `dist`, `build`, `_site` ya da `out` | — |

Paket yöneticisi (npm, pnpm, yarn ya da bun) kilit dosyanıza bakılarak seçilir.

### Projeye özel dosya: `bozkurt.toml`

Site ayarlarını ekibinizle paylaşmak için bu dosyayı depoya ekleyin. Elle verdiğiniz ayarlar dosyadakileri, dosyadakiler
de otomatik tanımayı geçersiz kılar.

```toml
domain = "magaza"           # → magaza.test
type = "php"                # php | proxy | static | app
root = "public"
php = "8.3"
node = "22"
command = "npm run dev"     # type = "app" için
aliases = ["yonetim.magaza"]
services = ["mariadb", "redis", "mailpit"]

[env]
APP_ENV = "local"
```

### Siteleri değiştirme, başlatma ve durdurma

- Uygulamadaki **Site ayrıntısı** ekranında alan adını, takma adları, kök klasörü, PHP/Node sürümünü, ortam
  değişkenlerini, HTTPS'i ve joker alt alan adlarını değiştirebilirsiniz. `app` sitelerinde ayrıca **Başlat / Durdur /
  Yeniden başlat** düğmeleri ve canlı günlük vardır.
- `bz open` bulunduğunuz klasörün sitesini tarayıcıda açar. `bz sites` tüm siteleri listeler.
- `bz unlink` siteyi BOZKURT'tan kaldırır. **Proje dosyalarınıza asla dokunulmaz.**

### Özel Caddy kuralları

BOZKURT, `.htaccess` dosyalarını okumayan Caddy'yi kullanır. WordPress, Laravel, Symfony ve genel ön denetleyici
(front controller) siteleri için eşdeğer kurallar otomatik üretilir. Başka ihtiyaçlarınız için
`$BOZKURT_HOME/caddy/snippets/<alan-adı>.caddy` dosyasını oluşturun. Dosya, o sitenin bloğunun sonunda içe aktarılır:

```caddy
# ~/.bozkurt/caddy/snippets/magaza.test.caddy
header /assets/* Cache-Control "no-store"
redir /eski-blog/* /blog{uri} permanent
```

<a id="bolum-5"></a>

## 5. Çalışma ortamları

**Uygulamada:** **Çalışma ortamları** ekranında bir sekme seçin (PHP, Node.js, Python, Bun, Go) ve bir sürümde **Kur** ya
da **Kullan**'a basın. Liste kaynaktan canlı gelir; LTS, güncel ve desteği bitmiş (EOL) sürümler etiketlidir.

**CLI ile:**

```sh
bz php list --available
bz php install 8.4
bz php use 8.4            # genel varsayılan
bz node install lts
bz python install 3.12
bz go install latest
bz php uninstall 8.1
```

**macOS ve Linux'ta PHP: "Sağlama toplamı olmadan kurulsun mu?"** macOS ve Linux için PHP,
derlemeleri için sağlama toplamı (checksum) yayımlamayan [static-php.dev](https://static-php.dev)'den gelir. Bu yüzden BOZKURT
indirilen dosyayı doğrulayamaz ve varsayılan olarak reddeder. Uygulama bunu açıklayan bir pencere gösterir; dosyaya bir kez
güvenmek için **Yine de kur**'u seçin. Dosyanın SHA-256 değeri o andan itibaren **sabitlenir**: aynı dosyanın sonraki her
indirmesi bununla birebir eşleşmek zorundadır, yoksa reddedilir. CLI'da kurulum `unverified download: …` hatasıyla biter;
komutu bayrakla yeniden çalıştırın:

```sh
bz php install 8.4 --allow-unverified
```

Bu şekilde bir kez kurduğunuz sürüm yeniden kurulurken bir daha sormaz. Tüm doğrulanmamış indirmelere izin vermek için
`config.toml`'da `allow_unverified = true` ayarlayın (önerilmez). Windows PHP derlemeleri normalde sağlama toplamıyla gelir ve
sormadan kurulur.

**Siteye özel sürümler.** Site, ayarlarında ya da `bozkurt.toml` dosyasında sabitlenmiş sürümü kullanır. İkisinde de sürüm
yoksa proje dosyalarına bakılır:

| Çalışma ortamı | Okunduğu yer |
|---|---|
| PHP | `composer.json` → `require.php` |
| Node.js | `.nvmrc`, `.node-version`, `package.json` → `engines.node` |
| Python | `.python-version` |
| Go | `go.mod` |

**Terminalde:** `$BOZKURT_HOME/bin` altındaki ara komutlar (`php`, `composer`, `node`, `npm`, `npx`, `bun`, `python`, `pip`,
`go`) bir proje klasörünün içindeyseniz projenin sürümünü, değilseniz genel sürümü çalıştırır. `composer` otomatik
indirilir ve doğrulanır.

<a id="bolum-6"></a>

## 6. Servisler ve veritabanları

**Uygulamada:** **Servisler** ekranı her servisin durumunu, portunu, sürümünü ve çalışma süresini gösterir. Başlat,
Durdur, Yeniden başlat, otomatik başlatma ve günlük düğmeleri de buradadır.
**CLI ile:** `bz start mariadb`, `bz stop all`, `bz restart caddy`, `bz status`.

| Servis | Varsayılan port | Notlar |
|---|---|---|
| caddy | 80 / 443 (yedek 8080 / 8443) | Tüm sitelerin web sunucusu |
| dns | 5390 (UDP+TCP) | `*.test` sorgularını yanıtlar |
| mariadb | 3306 | Kullanıcı `root`, parola yok (yalnızca yerel geliştirme için). Şimdilik Linux x64 ve Windows x64 için paketli. |
| postgres | 5432 | Kullanıcı `postgres`, localhost'ta parolasız (trust) |
| redis | 6379 | Valkey. Şimdilik Linux için paketli. |
| mailpit | SMTP 1025, web arayüzü 8025 | Uygulamanızın e-posta ayarını `127.0.0.1:1025` yapın, gelen postaları `http://127.0.0.1:8025` adresinde görün |
| php-fpm-X.Y | soket (Windows'ta TCP 9000+X·10+Y, ör. 8.3 → 9083) | Kullanılan her PHP sürümü için bir havuz |

**Zaten MySQL, PostgreSQL ya da Redis kullanıyor musunuz?** Varsayılan portta başka bir program dinliyorsa servis
**external** (harici) olarak görünür ve BOZKURT onu verisine ya da ayarlarına dokunmadan olduğu gibi kullanır. BOZKURT'un
henüz veritabanı paketlemediği bir platformda veritabanı kullanmanın yolu da budur.

**Veritabanları:** `bz db create magaza`, `bz db list`, `bz db drop magaza` (onay ister).

**Laravel `.env` örneği:**

```dotenv
DB_CONNECTION=mariadb
DB_HOST=127.0.0.1
DB_PORT=3306
DB_DATABASE=magaza
DB_USERNAME=root
DB_PASSWORD=
MAIL_HOST=127.0.0.1
MAIL_PORT=1025
REDIS_HOST=127.0.0.1
```

<a id="bolum-7"></a>

## 7. HTTPS ve yerel sertifika otoritesi

- BOZKURT, `$BOZKURT_HOME/ca/` altında kendi sertifika otoritesini oluşturur. Özel anahtarı yalnızca siz okuyabilirsiniz.
- CA, kullanıcı depolarınıza eklenir:
  - macOS: **giriş (login)** anahtar zinciriniz;
  - Windows: **CurrentUser\Root**;
  - Linux: NSS veritabanlarınız (Chrome/Chromium ve Firefox profilleri).

  `bz trust` onu ayrıca sistem deposuna da ekler; bunun için yönetici yetkisi gerekir (işletim sistemi parolanızı sorar).
- HTTPS açık her siteye, alan adını ve takma adlarını (joker açıksa `*.alan-adı` adresini de) kapsayan bir sertifika
  üretilir. Sertifikalar süreleri dolmadan kendiliğinden yenilenir.
- HTTPS'i siteye göre **Site ayrıntısı** ekranından ya da `bz secure` / `bz unsecure` ile açıp kapatın. Açıkken HTTP
  istekleri HTTPS'e yönlendirilir.
- İşletim sistemi deposunu kullanmayan komut satırı araçlarına (bazı `curl` sürümleri, Node.js, Python `requests`) CA'yı
  göstermeniz gerekir:
  ```sh
  export NODE_EXTRA_CA_CERTS="$HOME/.bozkurt/ca/rootCA.pem"
  export SSL_CERT_FILE="$HOME/.bozkurt/ca/rootCA.pem"   # dikkat: o süreç için varsayılan sertifika paketinin yerine geçer
  ```
- `bz trust --undo`, CA'yı eklendiği tüm depolardan kaldırır.

<a id="bolum-8"></a>

## 8. Yerel DNS

- Dahili DNS yanıtlayıcı, TLD'nizin altındaki her adı (`magaza.test`, `api.magaza.test`…) `127.0.0.1` ve `::1` ile yanıtlar.
- `bz dns setup`, işletim sisteminize `.test` sorgularını bu yanıtlayıcıya göndermesini söyler:

  | İşletim sistemi | Yöntem | Yedek yöntem |
  |---|---|---|
  | macOS | `/etc/resolver/test` | — |
  | Linux | systemd-resolved eki ya da NetworkManager'ın dnsmasq'ı | `/etc/hosts` içinde yönetilen bir blok |
  | Windows | `.test` için NRPT kuralı (bu durumda yanıtlayıcı 53. portu dinlemelidir) | hosts dosyasında yönetilen bir blok |

  Hosts dosyası joker kayıt desteklemez; bu yüzden yedek yöntemde her site ve takma ad için ayrı satır yazılır.
- **Windows: 53. port dolu.** NRPT, yanıtlayıcının 53. portu dinlemesini gerektirir. Bu portu başka bir program
  tutuyorsa (çoğunlukla İnternet Bağlantısı Paylaşımı, Hyper-V/WSL ya da Docker Desktop), `bz dns setup` hosts dosyasını
  kullanır ve nedenini programın adıyla birlikte söyler, örneğin `127.0.0.1:53 is in use by svchost.exe (pid 1234) …
  Sites are listed in the hosts file instead; wildcard subdomains do not resolve.` NRPT'ye geçmek için 53. portu
  boşaltıp `bz dns setup` komutunu yeniden çalıştırın.
- **TLD'yi değiştirmek için:** Ayarlar → Genel → TLD, ya da `bz config set tld local`. `.dev` ve `.app` kabul edilmez,
  çünkü HTTPS'i zorunlu kılan gerçek alan adlarıdır. Değiştirdikten sonra `bz dns setup` komutunu yeniden çalıştırın.
  **TLD değişikliğinden sonra CA'ya yeniden güvenin.** Yerel CA yalnızca `.test` ve `localhost` için sertifika imzalar (böylece
  gerçek web siteleri için kötüye kullanılamaz). Başka bir TLD'ye geçtiğinizde BOZKURT CA'yı bu TLD'yi de kapsayacak şekilde
  yeniler ve site sertifikalarını yeniden üretir; yeni CA'ya güvenene kadar tarayıcılar uyarı verir. Uygulama bir **CA'ya güven**
  düğmesi gösterir, ya da `bz trust` çalıştırın. Gerçek bir genel TLD yerine ayrılmış bir ad (`test`, `localhost` ya da `loc`
  gibi uydurma bir ad) seçin.
- `bz dns setup --undo`, önceki ayarları yedekten geri yükler.

<a id="bolum-9"></a>

## 9. Günlükler, işler ve Doktor

- **Günlükler:** Günlükler ekranı tüm servisleri ve siteleri canlı ve süzülebilir biçimde gösterir. CLI: `bz logs caddy -f`,
  `bz logs magaza.test -n 500`. Dosyalar `$BOZKURT_HOME/logs/` altında saklanır.
- **İşler:** sağ alttaki İşler çekmecesi indirmeleri ve kurulum adımlarını ilerlemeleriyle gösterir. CLI: `bz jobs`.
- **Doktor:** Doktor ekranı ya da `bz doctor`; portları, DNS çözümlemesini, CA güvenini, `PATH`'i, izinleri ve servisleri
  denetler. Her uyarı ve hata için bir çözüm önerisi verir.

<a id="bolum-10"></a>

## 10. Bir siteyi herkese açmak

`bz share magaza.test` (ya da Site ayrıntısındaki **Paylaş** düğmesi), Cloudflare'in `cloudflared` aracıyla geçici ve
herkese açık bir adres oluşturur. Araç ilk kullanımda indirilir. Paylaşım açıkken adresi bilen herkes sitenize
erişebilir. Durdurmak için Ctrl-C'ye ya da **Paylaşımı durdur**'a basın.

<a id="bolum-11"></a>

## 11. Mobil eşlikçi

Telefon uygulaması (iOS/Android) bilgisayarınız için bir **uzaktan kumandadır**. Telefonlar PHP, MySQL ya da geliştirme
sunucusu çalıştıramaz; sunucu bilgisayarınızda kalır, telefon onu yerel ağ üzerinden yönetir.

1. **Masaüstünde:** Ayarlar → **Uzaktan erişim** → etkinleştirin. Bu, 7879 numaralı portta bir dinleyici başlatır;
   dinleyici varsayılan olarak kapalıdır. **Eşleştirme kodu oluştur**'a basın. 5 dakika geçerli, tek kullanımlık bir QR
   kod ve 8 haneli bir kod (`1234 5678` biçiminde) görünür.
2. **Telefonda:** uygulamayı açıp **QR kodu tara**'ya basın ya da `<bilgisayar-ip>:7879` adresini ve kodu yazın.
3. Bu kadar. Telefona kendine ait bir erişim anahtarı verilir. Bu anahtarı istediğiniz zaman Ayarlar → Uzaktan erişim →
   eşleşmiş cihazlar bölümünden ya da `bz remote revoke <id>` ile iptal edebilirsiniz.

İki cihaz da aynı ağda olmalıdır. Bağlantı kurulamıyorsa bilgisayarınızın güvenlik duvarında 7879 numaralı porta izin verin.
Uzaktan erişimi açarken portun kullanımda olduğu söylenirse uzaktan erişim kapalı kalır; 7879 numaralı portu boşaltın ya da
`bz config set remote.port 7880` ile başka bir port seçin.

Yanlış kod denemeleri sınırlıdır: bir telefondan dakikada 5 denemeden (tüm cihazlardan toplam 30) sonra bir dakika beklemeniz
gerekir; toplam 10 yanlış koddan sonra kod iptal edilir. Bilgisayarda yeni bir kod oluşturup onu girin. Telefon ile bilgisayar
arasındaki bağlantı henüz şifreli değildir; uzaktan erişimi yalnızca güvendiğiniz ağlarda kullanın.
Uygulamadaki **Demoyu dene** seçeneği bilgisayar olmadan da çalışır.

<a id="bolum-12"></a>

## 12. Yapay zekâ ajanları (MCP)

MCP sunucusunu editörünüze ya da ajanınıza tanıtın:

```sh
bz mcp register claude-code     # ya da: cursor, vscode, windsurf, gemini, codex
```

Sonra "bu depoyu HTTPS ve PHP 8.3 ile bağla" ya da "magaza.test neden 502 veriyor?" gibi isteklerde bulunun. Veri silen,
işletim sistemine dokunan ya da bir siteyi herkese açan araçlar önce bir **önizleme** döndürür. Ajan bunu size göstermeli
ve ancak onayınızdan sonra `confirm: true` ile yeniden çağırmalıdır. Bir sitenin kabuk komutunu ya da ortam değişkenlerini
ayarlayan veya çalıştıran her işlem için de aynısı geçerlidir: komut çalışmadan önce tam olarak ne çalışacağını görürsünüz. Uzaktan erişim, eşleştirme ve içe aktarma ajanlara
açık değildir. Ayrıntılar için [MCP.md](MCP.md).

<a id="bolum-13"></a>

## 13. Diğer araçlardan geçiş

**Uygulamada:** Ayarlar → **İçe aktar**. **CLI ile:** `bz import`.

BOZKURT, Laravel Valet, Laravel Herd, MAMP, XAMPP, Laragon ve Homebrew'daki (Apache/nginx/dnsmasq) siteleri ve DNS
ayarlarını tarar ve bulduklarını listeler. Yalnızca seçtikleriniz içe aktarılır. `$BOZKURT_HOME` dışında değiştirilen
her dosya önce `$BOZKURT_HOME/backups/<zaman>/` altına yedeklenir. DNS ayarları listelenir ama **hiçbir zaman içe
aktarılmaz**: BOZKURT başka bir aracın çözümleyicisine dokunmaz. İçe aktarma sonucu bunların her birini "DNS not
imported" başlığı altında sayar ve eski bir çözümleyici sizin TLD'nizi de yönetiyorsa uyarır (örneğin Valet'in
`/etc/resolver/test` dosyası). O dosyayı kendiniz silin; aksi hâlde eski araç sitelerinizi yanıtlamaya devam edebilir.
Yalnızca DNS kayıtlarını seçerseniz hiçbir şey içe aktarılmaz.

<a id="bolum-14"></a>

## 14. Ayarlar ve dosyalar

**Ayarlar ekranı:** dil (English / Türkçe), tema (açık/koyu/sistem), TLD, portlar, ağ (siteleri yerel ağda yayınlama;
doğrulanmamış indirmelere izin verilip verilmediği), uzaktan erişim ve içe aktarma. Portlar 1 ile 65535 arasında olmalı ve
API ile uzaktan erişim portları dahil hepsi birbirinden farklı olmalıdır; arka plan hizmetinin mesajı ilgili alanın altında
görünür.

Değerleri `bz config set <anahtar> <değer>` ile de değiştirebilirsiniz (`remote.port` gibi noktalı anahtarlar).

`$BOZKURT_HOME/config.toml` (varsayılan değerlerle):

```toml
tld = "test"
dns_port = 5390
http_port = 80          # kullanılamazsa 8080
https_port = 443        # kullanılamazsa 8443
api_port = 7878         # arka plan hizmetinin (daemon) API'si, yalnızca 127.0.0.1
autostart = ["caddy", "dns"]
lan_access = false      # true: siteler yalnızca 127.0.0.1 / ::1 yerine tüm ağ arayüzlerinde sunulur
allow_unverified = false  # true: sağlama toplamı yayımlanmamış her indirmeyi kabul et (ilk indirmede sabitlenir)

[remote]
enabled = false
port = 7879
```

Klasör yapısı:

```
bin/        çalışma ortamı ara komutları (php, composer, node, npm, npx, corepack, bun, bunx, python, python3, pip, pip3,
            go, gofmt); CLI kurulum betiği bz, bozkurtd ve bozkurt-mcp'yi de buraya koyar
runtimes/   <tür>/<sürüm>/           current/<tür> → etkin sürüm
services/   <ad>/ ikili dosyalar ve veriler (ör. services/mariadb/data)
caddy/      Caddyfile (üretilir), bin/, data/, snippets/<alan-adı>.caddy
ca/         rootCA.pem, rootCA-key.pem
certs/      <alan-adı>.pem, <alan-adı>-key.pem
logs/       <bileşen>.log
cache/      kaynak sürüm listeleri (6 saat)
tmp/        süren indirmeler
backups/    <zaman>/ bu klasörün dışında değiştirilen her şeyin kopyası
config.toml, state.json, api-token (yerel API anahtarı, 0600), bozkurtd.pid
```

**Klavye:** ⌘K / Ctrl K komut paletini açar, `?` tüm kısayolları listeler.

<a id="bolum-15"></a>

## 15. Kaldırma

1. `bz dns setup --undo` ve `bz trust --undo` komutlarını çalıştırın. İşletim sistemindeki DNS yönlendirmesi ve CA güveni
   kaldırılır, yedekler geri yüklenir.
2. `bz stop all` deyip uygulamadan çıkın.
3. Uygulamayı silin (Uygulamalar klasörü, Programlar ve Özellikler ya da paket yöneticiniz).
4. `$BOZKURT_HOME` klasörünü silin. **BOZKURT'un yönettiği veritabanlarınız da silinir**, bu yüzden önce ihtiyacınız
   olanları dışa aktarın.

<a id="bolum-16"></a>

## 16. Sorun giderme

| Belirti | Deneyin |
|---|---|
| `magaza.test` çözümlenmiyor | `bz doctor`, ardından `bz dns setup`. Linux'ta özel bir çözümleyici kullanıyorsanız systemd-resolved ekinin ya da hosts bloğunun var olduğunu kontrol edin. Windows'ta NRPT için 53. port boş olmalı; değilse hosts dosyası kullanılır. |
| Tarayıcı sertifika uyarısı veriyor | `bz trust`, ardından tarayıcıyı yeniden başlatın. Linux'ta Firefox kullanıyorsanız profilin NSS veritabanının güncellendiğinden emin olun. `bz trust status` ile denetleyin. |
| 80/443 portları dolu | `bz doctor` portu tutan süreci söyler. Onu durdurun ya da 8080/8443'ü kabul edin (adresler `http://magaza.test:8080` olur). |
| `app` sitesinde 502 hatası | Geliştirme komutu çökmüş ya da yanlış portu dinliyor. `bz logs magaza.test` ile bakın. Komutunuz `$PORT` değişkenini kullanmalı. 1.0'dan önce oluşturulmuş bir Vite / SvelteKit / Astro sitesinin komutuna `--host 127.0.0.1` eklemeniz gerekebilir (yeni sitelere otomatik eklenir). |
| Windows: yalnızca tam site adları çözümleniyor, `*.magaza.test` çözümlenmiyor | 53. port dolu olduğu için DNS hosts dosyasını kullanıyor. `bz dns setup` portu tutan programı söyler; portu boşaltıp komutu yeniden çalıştırın. |
| Windows: tırnak içindeki bir `npm` / `npx` argümanında `^` iki kez geliyor | 1.0'da `npm`, `npx` ve `corepack` için bilinen bir sınır (bunlar `.cmd` dosyalarıdır). Tırnak içinde `^` kullanmayın: örneğin `"react@~18"` ya da tam sürüm yazın veya aralığı `package.json` içine koyun. `node`, `php`, `composer`, `python`, `go` ve `bun` etkilenmez. |
| "PHP X kurulu değil" sayfası | `bz php install X` (ya da sitenin PHP sürümünü değiştirin). |
| MariaDB `external` görünüyor | :3306'yı başka bir MySQL/MariaDB tutuyor. BOZKURT onu kullanıyor; bağlanırken *onun* kullanıcı bilgilerini kullanın. |
| `bz: daemon unreachable` (çıkış kodu 3) | Arka plan hizmetine ulaşılamıyor: masaüstü uygulamasını açın ya da `bz daemon` çalıştırın. `$BOZKURT_HOME/logs/bozkurtd.log` dosyasına bakın. |
| Bir betikten `401` / "missing local API token" | `Authorization: Bearer $(cat ~/.bozkurt/api-token)` başlığını gönderin. `bz` kullanıyorsanız `--home` / `BOZKURT_HOME` değerinin çalışan arka plan hizmetininkiyle aynı olduğundan emin olun. |
| Telefon bağlanamıyor | Aynı Wi-Fi ağında mısınız? Uzaktan erişim açık mı? Güvenlik duvarı 7879'a izin veriyor mu? Eşleştirme kodu 5 dakikadan eski olmasın. |
| "Çok fazla yanlış kod girildi, eşleştirme kodu iptal edildi" | Bilgisayarda yeni bir eşleştirme kodu oluşturup onu kullanın. |
| PHP kurulumu "unverified download" ile başarısız oluyor | macOS/Linux'ta beklenen bir durum: **Yine de kur**'u seçin ya da `bz php install <sürüm> --allow-unverified` çalıştırın. Bkz. [Çalışma ortamları](#bolum-5). |
| "ilk indirmede sabitlenen kopyadan farklı" | Dosya ilk kurulumdan bu yana değişmiş ve bilerek reddedildi; sürekli oluyorsa bildirin. |
| `config.toml` düzenledikten sonra `bozkurtd` başlamıyor | Bir port aralık dışında ya da iki kez kullanılmış; kayıt dosyası anahtarı söyler (`https_port (80) is already used by http_port`). |

Hâlâ çözemediyseniz `bz doctor --json` çıktısıyla bir issue açın (önce kişisel klasör yollarını silin).
