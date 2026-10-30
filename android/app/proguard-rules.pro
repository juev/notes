# The Rust core is reached through JNA: native methods are bound by name and
# structures are read by reflection, so neither JNA nor the generated UniFFI
# bindings may be renamed or stripped.
-keep class com.sun.jna.** { *; }
-keep class * implements com.sun.jna.** { *; }
-keep class uniffi.** { *; }
-dontwarn java.awt.**

# WorkManager creates its Room database by reflection; R8 in full mode drops
# the constructor it looks for.
-keep class * extends androidx.room.RoomDatabase { <init>(); }
