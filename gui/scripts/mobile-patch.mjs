// Put back what `tauri android init` / `tauri ios init` cannot know about.
//
// `src-tauri/gen/` is generated and gitignored, and two things live in it that
// the generator does not produce: Arvolo's place in each platform's share sheet
// (an intent filter plus a Kotlin object on Android; a share-extension target
// plus App Group entitlements on iOS), and the iOS script-phase path fix this
// checkout needs. Their sources are tracked under `src-tauri/mobile/`; this
// script copies them into place and regenerates the Xcode project. Run it after
// every `tauri … init`, and before a build when in doubt — it is idempotent.
//
//   npm run mobile:patch            # both platforms, whichever gen/ exists
//   npm run mobile:patch -- android
//   npm run mobile:patch -- ios

import { cpSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { execSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const tauri = join(here, "..", "src-tauri");
const mobile = join(tauri, "mobile");
const only = process.argv[2];

function android() {
  const gen = join(tauri, "gen", "android");
  if (!existsSync(gen)) return console.log("android: no gen/android (run `tauri android init` first)");
  const pkg = join(gen, "app", "src", "main", "java", "it", "termox", "arvolo");
  mkdirSync(pkg, { recursive: true });
  for (const f of ["MainActivity.kt", "Shared.kt", "Photos.kt"]) {
    cpSync(join(mobile, "android", f), join(pkg, f));
  }
  const manifestPath = join(gen, "app", "src", "main", "AndroidManifest.xml");
  let manifest = readFileSync(manifestPath, "utf8");
  // Sempre in piedi. Il generatore dichiara l'activity senza orientamento, cioè
  // libera di ruotare, e le schermate sono disegnate per una colonna sola: da
  // coricate non diventano una versione larga, diventano la stessa colonna con
  // metà dell'altezza. Lo stesso vincolo che `project.yml` mette su iOS.
  if (!manifest.includes("android:screenOrientation")) {
    const at = manifest.indexOf('android:name=".MainActivity"');
    if (at < 0) throw new Error("AndroidManifest.xml: MainActivity not found");
    manifest =
      manifest.slice(0, at) +
      'android:screenOrientation="portrait"\n            ' +
      manifest.slice(at);
    writeFileSync(manifestPath, manifest);
  }
  // Fra due marcatori, e **riscritto ogni volta**.
  //
  // Prima la condizione era «il manifest contiene già SEND_MULTIPLE?»: vera
  // dopo il primo giro, quindi una riga aggiunta più tardi al file dei filtri
  // non entrava mai più in un `gen/` già patchato. Un patch che non si accorge
  // di essere vecchio è peggio di uno che non c'è: il sorgente dice una cosa e
  // l'app ne fa un'altra, e nessuno dei due mente in modo visibile.
  const BEGIN = "<!-- arvolo:filters:begin -->";
  const END = "<!-- arvolo:filters:end -->";
  const filter = readFileSync(join(mobile, "android", "share-intent-filter.xml"), "utf8");
  const block = `${BEGIN}\n${filter.trimEnd()}\n            ${END}`;
  const from = manifest.indexOf(BEGIN);
  if (from >= 0) {
    const to = manifest.indexOf(END, from);
    if (to < 0) throw new Error("AndroidManifest.xml: unterminated arvolo:filters block");
    manifest = manifest.slice(0, from) + block + manifest.slice(to + END.length);
  } else {
    // Inside the main activity, after its launcher filter.
    const at = manifest.indexOf("</intent-filter>", manifest.indexOf("android.intent.category.LAUNCHER"));
    if (at < 0) throw new Error("AndroidManifest.xml: launcher intent-filter not found");
    const cut = at + "</intent-filter>".length;
    manifest = manifest.slice(0, cut) + "\n            " + block + manifest.slice(cut);
  }
  writeFileSync(manifestPath, manifest);
  // Le regole R8 che il generatore non può conoscere (vedi il file).
  cpSync(
    join(mobile, "android", "proguard-arvolo.pro"),
    join(gen, "app", "proguard-arvolo.pro")
  );

  // La firma di release, che il generatore non mette perché non sa dove tieni
  // la chiave. Senza, `tauri android build --apk` sforna un APK **non firmato**,
  // che nessun telefono installa — e lo fa senza lamentarsi.
  //
  // Il file delle proprietà sta in `.secrets/`, fuori dai commit (questo repo è
  // pubblico); qui dentro si scrive solo come leggerlo. Il blocco è fra due
  // marcatori e viene riscritto ogni volta, come quello del manifest.
  const gradlePath = join(gen, "app", "build.gradle.kts");
  let gradle = readFileSync(gradlePath, "utf8");
  const G_BEGIN = "// arvolo:signing:begin";
  const G_END = "// arvolo:signing:end";
  const signing = `${G_BEGIN}
    signingConfigs {
        create("release") {
            // Properties() e non java.util.Properties(): dentro un blocco
            // Gradle, java è l'estensione del plugin Java e non il package, e
            // oscura il nome. L'import sta già in cima al file generato.
            val props = Properties()
            val f = rootProject.file("../../../../.secrets/android-release.properties")
            if (f.exists()) {
                f.inputStream().use { props.load(it) }
                storeFile = rootProject.file(props.getProperty("storeFile"))
                storePassword = props.getProperty("storePassword")
                keyAlias = props.getProperty("keyAlias")
                keyPassword = props.getProperty("keyPassword")
            }
        }
    }
    ${G_END}`;
  const gFrom = gradle.indexOf(G_BEGIN);
  if (gFrom >= 0) {
    const gTo = gradle.indexOf(G_END, gFrom);
    gradle = gradle.slice(0, gFrom) + signing + gradle.slice(gTo + G_END.length);
  } else {
    const at = gradle.indexOf("    buildTypes {");
    if (at < 0) throw new Error("build.gradle.kts: buildTypes not found");
    gradle = gradle.slice(0, at) + signing + "\n" + gradle.slice(at);
  }
  // E il tipo release deve usarla.
  if (!gradle.includes('signingConfig = signingConfigs.getByName("release")')) {
    gradle = gradle.replace(
      'getByName("release") {',
      'getByName("release") {\n            signingConfig = signingConfigs.getByName("release")'
    );
  }
  writeFileSync(gradlePath, gradle);

  console.log("android: share target in place");
}

function ios() {
  const gen = join(tauri, "gen", "apple");
  if (!existsSync(gen)) return console.log("ios: no gen/apple (run `tauri ios init` first)");
  cpSync(join(mobile, "ios", "ShareExtension"), join(gen, "ShareExtension"), { recursive: true });
  cpSync(join(mobile, "ios", "arvolo-gui_iOS.entitlements"), join(gen, "arvolo-gui_iOS", "arvolo-gui_iOS.entitlements"));
  cpSync(join(mobile, "ios", "project.yml"), join(gen, "project.yml"));
  // The .xcodeproj is what xcodebuild reads; project.yml is only its source.
  execSync("xcodegen generate", { cwd: gen, stdio: "inherit" });
  console.log("ios: share extension and entitlements in place, project regenerated");
}

if (!only || only === "android") android();
if (!only || only === "ios") ios();
