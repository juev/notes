plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.plugin.compose")
}

// The same variable as in scripts/build-android.sh: the phone needs arm64-v8a,
// the CI check of the release runs on an x86_64 emulator.
val releaseAbis = providers.environmentVariable("NOTES_ANDROID_ABIS").orNull
    ?.split(" ")?.filter { it.isNotBlank() } ?: listOf("arm64-v8a")

android {
    namespace = "org.evsyukov.notes"
    compileSdk = 37

    defaultConfig {
        applicationId = "org.evsyukov.notes"
        minSdk = 26
        targetSdk = 36
        // Any published build raises versionCode, a release candidate as well.
        versionCode = 1
        versionName = "0.1.0"
    }

    buildTypes {
        // The ABIs must be among those built by scripts/build-android.sh.
        debug {
            ndk { abiFilters += listOf("arm64-v8a", "x86_64") }
        }
        // Nothing is published yet, so the release is signed with the debug key of the machine:
        // it goes through R8 and runs at full speed on a phone (make android-install).
        release {
            ndk { abiFilters += releaseAbis }
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            signingConfig = signingConfigs.getByName("debug")
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    buildFeatures {
        compose = true
    }
    // E40: the same font files as the macOS app.
    androidResources {
        noCompress += "ttf"
    }
    sourceSets {
        getByName("main") {
            assets.directories.add("build/generated/fontAssets")
        }
    }
    testOptions {
        unitTests.isIncludeAndroidResources = true
    }
}

kotlin {
    compilerOptions {
        jvmTarget = org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17
    }
}

tasks.withType<Test>().configureEach {
    systemProperty("jna.library.path", rootDir.resolve("../core/target/debug").absolutePath)
}

val copyFonts by tasks.registering(Copy::class) {
    from("../../apple/App/Fonts") { include("*.ttf", "OFL-*.txt") }
    into(layout.buildDirectory.dir("generated/fontAssets/fonts"))
}
tasks.named("preBuild") { dependsOn(copyFonts) }

dependencies {
    val composeBom = platform("androidx.compose:compose-bom:2026.09.00")
    implementation(composeBom)
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.material:material-icons-extended")
    implementation("androidx.activity:activity-compose:1.11.0")
    implementation("androidx.lifecycle:lifecycle-viewmodel-compose:2.9.4")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.9.4")
    implementation("androidx.work:work-runtime-ktx:2.10.4")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.10.2")
    // Runtime of the UniFFI bindings to the Rust core.
    implementation("net.java.dev.jna:jna:5.17.0@aar")
    testImplementation("junit:junit:4.13.2")
    testImplementation("org.robolectric:robolectric:4.17")
    // The tests reach the core built for this machine through the same bindings.
    testImplementation("net.java.dev.jna:jna:5.17.0")
}
