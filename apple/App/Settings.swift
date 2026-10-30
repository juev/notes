import SwiftUI

/// A moment the core gives as text, for the settings.
enum Stamp {
    static func label(_ moment: String) -> String {
        let formats = [ISO8601DateFormatter()]
        for format in formats {
            if let date = format.date(from: moment) { return date.formatted(date: .abbreviated, time: .shortened) }
        }
        let plain = DateFormatter()
        plain.locale = Locale(identifier: "en_US_POSIX")
        plain.dateFormat = "yyyy-MM-dd'T'HH:mm"
        return plain.date(from: String(moment.prefix(16))).map { $0.formatted(date: .abbreviated, time: .shortened) } ?? moment
    }
}

/// The settings of the app (R80).
struct SettingsView: View {
    @Environment(AppModel.self) private var model
    @AppStorage("settingsTab", store: Storage.defaults) private var tab = "general"

    var body: some View {
        TabView(selection: $tab) {
            GeneralSettings()
                .tabItem { Label(L("General"), systemImage: "gearshape") }
                .tag("general")
            AppearanceSettings()
                .tabItem { Label(L("Appearance"), systemImage: "textformat") }
                .tag("appearance")
            SyncSettings()
                .tabItem { Label(L("Sync"), systemImage: "arrow.triangle.2.circlepath") }
                .tag("sync")
            Form { BackupsSection() }
                .formStyle(.grouped)
                .tabItem { Label(L("Backups"), systemImage: "externaldrive") }
                .tag("backups")
        }
        .frame(width: 540)
    }
}

/// The days in the Trash, the folder of new notes, spelling, the log.
private struct GeneralSettings: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Form {
            Picker(L("Theme"), selection: Binding(get: { model.theme }, set: { model.theme = $0 })) {
                Text(L("System")).tag("system")
                Text(L("Light")).tag("light")
                Text(L("Dark")).tag("dark")
            }
            Stepper(L("Days in the Trash: %d", Int(model.settings.trashDays)), value: setting(\.trashDays), in: 1...3650)
            Picker(L("New notes go to"), selection: setting(\.newNoteFolder)) {
                Text(L("Outside any folder")).tag("root")
                Text(L("The last folder used")).tag("last")
                ForEach(model.folders, id: \.id) { folder in
                    Text(model.path(of: folder.id).map(\.name).joined(separator: " / ")).tag(folder.id)
                }
            }
            Toggle(L("Smart quotes and dashes"), isOn: setting(\.smartTypography))
            Section(L("Spelling")) {
                Toggle(L("Check spelling"), isOn: setting(\.spellcheck))
                Picker(L("Language"), selection: setting(\.spellLanguage)) {
                    Text(L("Automatic")).tag("auto")
                    Text(L("Russian")).tag("ru")
                    Text(L("English")).tag("en")
                }
            }
            LogSection()
        }
        .formStyle(.grouped)
    }

    private func setting<T>(_ path: WritableKeyPath<AppSettings, T>) -> Binding<T> {
        Binding(get: { model.settings[keyPath: path] }, set: { value in
            var new = model.settings
            new[keyPath: path] = value
            model.saveSettings(new)
        })
    }
}

/// E40–E46: what every note looks like. The size and the column belong to this Mac.
private struct AppearanceSettings: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Form {
            Picker(L("Text font"), selection: setting(\.fontBody)) {
                ForEach(Typography.textFonts, id: \.self) { Text($0 == "system" ? L("System") : $0).tag($0) }
            }
            Picker(L("Code font"), selection: setting(\.fontMono)) {
                ForEach(Typography.codeFonts, id: \.self) { Text($0 == "system" ? L("System monospace") : $0).tag($0) }
            }
            Stepper(L("Size: %d pt", model.textSize), value: Binding(get: { model.textSize }, set: { model.textSize = $0 }), in: Typography.sizes)
            Picker(L("Line height"), selection: setting(\.lineHeight)) {
                ForEach(Typography.lineHeights, id: \.self) { Text(String(format: "%.1f", $0)).tag($0) }
            }
            Picker(L("Paragraph spacing"), selection: setting(\.paragraphSpacing)) {
                ForEach(Typography.paragraphSpacings, id: \.self) { Text(String(format: "%.2f", $0)).tag($0) }
            }
            Picker(L("Column width"), selection: Binding(get: { model.columnWidth }, set: { model.columnWidth = $0 })) {
                Text(L("Narrow")).tag(ColumnWidth.narrow)
                Text(L("Medium")).tag(ColumnWidth.medium)
                Text(L("Wide")).tag(ColumnWidth.wide)
                Text(L("Full width")).tag(ColumnWidth.full)
            }
            Picker(L("Alignment"), selection: setting(\.align)) {
                Text(L("Left")).tag(TextAlign.left)
                Text(L("Justified")).tag(TextAlign.justify)
            }
            Toggle(L("Hyphenation"), isOn: setting(\.hyphenate))
                .disabled(model.settings.align == .justify)
        }
        .formStyle(.grouped)
    }

    private func setting<T>(_ path: WritableKeyPath<AppSettings, T>) -> Binding<T> {
        Binding(get: { model.settings[keyPath: path] }, set: { value in
            var new = model.settings
            new[keyPath: path] = value
            model.saveSettings(new)
        })
    }
}
