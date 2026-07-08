diff --git a/chrome/browser/browseros/onboarding/browseros_onboarding.cc b/chrome/browser/browseros/onboarding/browseros_onboarding.cc
new file mode 100644
index 0000000000000..e936189f913b130160a04bbca0b871e09dafad54
--- /dev/null
+++ b/chrome/browser/browseros/onboarding/browseros_onboarding.cc
@@ -0,0 +1,728 @@
+// Copyright 2026 The Chromium Authors
+// Use of this source code is governed by a BSD-style license that can be
+// found in the LICENSE file.
+
+#include "chrome/browser/browseros/onboarding/browseros_onboarding.h"
+
+#include <stdint.h>
+
+#include <memory>
+#include <optional>
+#include <set>
+#include <string>
+#include <string_view>
+#include <utility>
+#include <vector>
+
+#include "base/functional/bind.h"
+#include "base/functional/callback.h"
+#include "base/location.h"
+#include "base/memory/weak_ptr.h"
+#include "base/notreached.h"
+#include "base/strings/stringprintf.h"
+#include "base/strings/utf_string_conversions.h"
+#include "base/task/sequenced_task_runner.h"
+#include "base/values.h"
+#include "chrome/browser/browser_process.h"
+#include "chrome/browser/importer/external_process_importer_host.h"
+#include "chrome/browser/importer/importer_list.h"
+#include "chrome/browser/importer/importer_progress_observer.h"
+#include "chrome/browser/importer/profile_writer.h"
+#include "chrome/browser/profiles/profile.h"
+#include "chrome/common/webui_url_constants.h"
+#include "chrome/grit/browseros_onboarding_resources.h"
+#include "chrome/grit/browseros_onboarding_resources_map.h"
+#include "components/user_data_importer/common/importer_data_types.h"
+#include "content/public/browser/visibility.h"
+#include "content/public/browser/web_contents.h"
+#include "content/public/browser/web_ui.h"
+#include "content/public/browser/web_ui_data_source.h"
+#include "content/public/browser/web_ui_message_handler.h"
+#include "content/public/common/url_constants.h"
+#include "ui/webui/webui_util.h"
+
+namespace {
+
+constexpr int kBrowserOSOnboardingApiVersion = 1;
+constexpr uint16_t kBrowserOSImportableItems =
+    user_data_importer::HISTORY | user_data_importer::FAVORITES |
+    user_data_importer::COOKIES | user_data_importer::PASSWORDS |
+    user_data_importer::SEARCH_ENGINES |
+    user_data_importer::AUTOFILL_FORM_DATA | user_data_importer::EXTENSIONS;
+
+std::string SourceIdForIndex(size_t index) {
+  return base::StringPrintf("source-%zu", index);
+}
+
+const char* ImportItemToString(user_data_importer::ImportItem item) {
+  switch (item) {
+    case user_data_importer::HISTORY:
+      return "history";
+    case user_data_importer::FAVORITES:
+      return "bookmarks";
+    case user_data_importer::COOKIES:
+      return "cookies";
+    case user_data_importer::PASSWORDS:
+      return "passwords";
+    case user_data_importer::SEARCH_ENGINES:
+      return "searchEngines";
+    case user_data_importer::AUTOFILL_FORM_DATA:
+      return "autofill";
+    case user_data_importer::EXTENSIONS:
+      return "extensions";
+    case user_data_importer::NONE:
+    case user_data_importer::HOME_PAGE:
+    case user_data_importer::ALL:
+      return nullptr;
+  }
+}
+
+uint16_t ImportItemMaskFromString(std::string_view item) {
+  if (item == "history") {
+    return user_data_importer::HISTORY;
+  }
+  if (item == "bookmarks") {
+    return user_data_importer::FAVORITES;
+  }
+  if (item == "cookies") {
+    return user_data_importer::COOKIES;
+  }
+  if (item == "passwords") {
+    return user_data_importer::PASSWORDS;
+  }
+  if (item == "searchEngines") {
+    return user_data_importer::SEARCH_ENGINES;
+  }
+  if (item == "autofill") {
+    return user_data_importer::AUTOFILL_FORM_DATA;
+  }
+  if (item == "extensions") {
+    return user_data_importer::EXTENSIONS;
+  }
+  return user_data_importer::NONE;
+}
+
+void AppendImportItem(base::ListValue& items,
+                      uint16_t services,
+                      user_data_importer::ImportItem item) {
+  if ((services & item) == 0) {
+    return;
+  }
+
+  const char* name = ImportItemToString(item);
+  if (name) {
+    items.Append(name);
+  }
+}
+
+base::ListValue ImportItemsFromMask(uint16_t services) {
+  base::ListValue items;
+  AppendImportItem(items, services, user_data_importer::HISTORY);
+  AppendImportItem(items, services, user_data_importer::FAVORITES);
+  AppendImportItem(items, services, user_data_importer::COOKIES);
+  AppendImportItem(items, services, user_data_importer::PASSWORDS);
+  AppendImportItem(items, services, user_data_importer::SEARCH_ENGINES);
+  AppendImportItem(items, services, user_data_importer::AUTOFILL_FORM_DATA);
+  AppendImportItem(items, services, user_data_importer::EXTENSIONS);
+  return items;
+}
+
+}  // namespace
+
+class BrowserOSOnboardingHandler : public content::WebUIMessageHandler,
+                                   public importer::ImporterProgressObserver {
+ public:
+  BrowserOSOnboardingHandler() = default;
+  BrowserOSOnboardingHandler(const BrowserOSOnboardingHandler&) = delete;
+  BrowserOSOnboardingHandler& operator=(const BrowserOSOnboardingHandler&) =
+      delete;
+  ~BrowserOSOnboardingHandler() override {
+    if (importer_host_) {
+      importer_host_->set_observer(nullptr);
+    }
+  }
+
+  void SetCompletionCallback(base::RepeatingClosure completion_callback) {
+    completion_callback_ = std::move(completion_callback);
+  }
+
+ private:
+  enum class ImportSourceResultStatus {
+    kPending,
+    kImporting,
+    kSucceeded,
+    kFailed,
+  };
+
+  struct ImportRequestSelection {
+    int source_index = 0;
+    std::string source_id;
+    uint16_t selected_items = user_data_importer::NONE;
+    bool has_selected_items = false;
+  };
+
+  struct ImportQueueEntry {
+    user_data_importer::SourceProfile source_profile;
+    std::string source_id;
+    std::string display_name;
+    uint16_t imported_items = user_data_importer::NONE;
+  };
+
+  struct ImportSourceResult {
+    std::string source_id;
+    std::string display_name;
+    ImportSourceResultStatus status = ImportSourceResultStatus::kPending;
+  };
+
+  void RegisterMessages() override {
+    web_ui()->RegisterMessageCallback(
+        "browserosOnboardingPageReady",
+        base::BindRepeating(&BrowserOSOnboardingHandler::HandlePageReady,
+                            base::Unretained(this)));
+    web_ui()->RegisterMessageCallback(
+        "browserosOnboardingRefreshSources",
+        base::BindRepeating(&BrowserOSOnboardingHandler::HandleRefreshSources,
+                            base::Unretained(this)));
+    web_ui()->RegisterMessageCallback(
+        "browserosOnboardingStartImport",
+        base::BindRepeating(&BrowserOSOnboardingHandler::HandleStartImport,
+                            base::Unretained(this)));
+    web_ui()->RegisterMessageCallback(
+        "browserosOnboardingComplete",
+        base::BindRepeating(&BrowserOSOnboardingHandler::HandleComplete,
+                            base::Unretained(this)));
+  }
+
+  void OnJavascriptDisallowed() override {
+    importer_list_.reset();
+    importer_list_loaded_ = false;
+    if (importer_host_) {
+      importer_host_->set_observer(nullptr);
+      importer_host_ = nullptr;
+    }
+    ResetImportState();
+  }
+
+  void HandlePageReady(const base::ListValue& args) {
+    if (!IsJavascriptAllowed()) {
+      AllowJavascript();
+    }
+    if (importer_host_) {
+      importer_host_->set_observer(nullptr);
+      importer_host_ = nullptr;
+    }
+    ResetImportState();
+    SendState("detecting");
+    DetectSources();
+  }
+
+  void HandleRefreshSources(const base::ListValue& args) {
+    if (IsImportQueueRunning()) {
+      SendFailure("importing", "import_in_progress",
+                  "An import is already in progress.");
+      return;
+    }
+
+    ResetImportState();
+    SendState("detecting");
+    DetectSources();
+  }
+
+  void HandleStartImport(const base::ListValue& args) {
+    if (IsImportQueueRunning()) {
+      SendFailure("importing", "import_in_progress",
+                  "An import is already in progress.");
+      return;
+    }
+
+    if (!CanStartImportFromCurrentWebContents()) {
+      SendFailure(
+          "ready", "user_interaction_required",
+          "Click Import from the visible onboarding window to continue.");
+      return;
+    }
+
+    ResetImportState();
+    if (!importer_list_loaded_ || !importer_list_ ||
+        importer_list_->count() == 0) {
+      SendFailure("no_sources", "No detected import source is ready.");
+      return;
+    }
+
+    std::vector<ImportRequestSelection> selections;
+    if (!BuildImportSelections(args, &selections)) {
+      SendFailure("invalid_source", "Selected import source is not valid.");
+      return;
+    }
+
+    std::vector<ImportQueueEntry> import_queue;
+    std::vector<ImportSourceResult> import_results;
+    bool has_supported_items = false;
+    for (const ImportRequestSelection& selection : selections) {
+      const user_data_importer::SourceProfile& source_profile =
+          importer_list_->GetSourceProfileAt(selection.source_index);
+      uint16_t imported_items =
+          GetEffectiveImportItems(source_profile, selection);
+      has_supported_items = has_supported_items || imported_items != 0;
+
+      std::string display_name = GetDisplayName(source_profile);
+      import_queue.push_back(
+          {source_profile, selection.source_id, display_name, imported_items});
+      import_results.push_back({selection.source_id, display_name,
+                                ImportSourceResultStatus::kPending});
+    }
+
+    if (!has_supported_items) {
+      SendFailure("no_supported_items",
+                  "Selected source has no supported import items.");
+      return;
+    }
+
+    import_queue_ = std::move(import_queue);
+    import_results_ = std::move(import_results);
+    import_queue_index_ = 0;
+    StartNextImport();
+  }
+
+  void HandleComplete(const base::ListValue& args) {
+    if (completion_handled_) {
+      return;
+    }
+    completion_handled_ = true;
+
+    SendState("completed");
+
+    if (completion_callback_) {
+      base::SequencedTaskRunner::GetCurrentDefault()->PostTask(
+          FROM_HERE, completion_callback_);
+    }
+  }
+
+  bool CanStartImportFromCurrentWebContents() {
+    content::WebContents* contents = web_ui()->GetWebContents();
+    return contents &&
+           contents->GetVisibility() == content::Visibility::VISIBLE &&
+           contents->HasRecentInteraction();
+  }
+
+  void DetectSources() {
+    importer_list_loaded_ = false;
+    ResetImportState();
+    importer_list_ = std::make_unique<ImporterList>();
+    importer_list_->DetectSourceProfiles(
+        g_browser_process->GetApplicationLocale(), false,
+        base::BindOnce(&BrowserOSOnboardingHandler::HandleSourcesDetected,
+                       base::Unretained(this)));
+  }
+
+  void HandleSourcesDetected() {
+    importer_list_loaded_ = true;
+    SendState("ready");
+  }
+
+  bool FindSourceIndex(const std::string& source_id, int* index) const {
+    for (size_t i = 0; importer_list_ && i < importer_list_->count(); ++i) {
+      if (SourceIdForIndex(i) == source_id) {
+        *index = static_cast<int>(i);
+        return true;
+      }
+    }
+    return false;
+  }
+
+  bool BuildImportSelections(
+      const base::ListValue& args,
+      std::vector<ImportRequestSelection>* selections) const {
+    if (!args.empty() && args[0].is_dict()) {
+      const base::DictValue& request = args[0].GetDict();
+      if (request.contains("selections")) {
+        const base::ListValue* requested_selections =
+            request.FindList("selections");
+        if (!requested_selections) {
+          return false;
+        }
+        return BuildImportSelectionsFromList(*requested_selections, selections);
+      }
+
+      ImportRequestSelection selection;
+      if (!BuildImportSelectionFromDict(request, &selection)) {
+        return false;
+      }
+      selections->push_back(std::move(selection));
+      return true;
+    }
+
+    std::optional<int> browser_index = args.empty() ? 0 : args[0].GetIfInt();
+    if (!browser_index) {
+      return false;
+    }
+    if (!IsValidSourceIndex(*browser_index)) {
+      return false;
+    }
+
+    selections->push_back(
+        {*browser_index, SourceIdForIndex(static_cast<size_t>(*browser_index)),
+         user_data_importer::NONE, false});
+    return true;
+  }
+
+  bool BuildImportSelectionsFromList(
+      const base::ListValue& requested_selections,
+      std::vector<ImportRequestSelection>* selections) const {
+    if (requested_selections.empty()) {
+      return false;
+    }
+
+    std::set<std::string> source_ids;
+    for (const base::Value& selection_value : requested_selections) {
+      if (!selection_value.is_dict()) {
+        return false;
+      }
+
+      ImportRequestSelection selection;
+      if (!BuildImportSelectionFromDict(selection_value.GetDict(),
+                                        &selection)) {
+        return false;
+      }
+      if (!source_ids.insert(selection.source_id).second) {
+        return false;
+      }
+      selections->push_back(std::move(selection));
+    }
+    return true;
+  }
+
+  bool BuildImportSelectionFromDict(const base::DictValue& request,
+                                    ImportRequestSelection* selection) const {
+    const std::string* source_id = request.FindString("sourceId");
+    if (!source_id || !FindSourceIndex(*source_id, &selection->source_index)) {
+      return false;
+    }
+
+    selection->source_id = *source_id;
+    if (const base::ListValue* items = request.FindList("items")) {
+      selection->has_selected_items = true;
+      for (const base::Value& item : *items) {
+        if (item.is_string()) {
+          selection->selected_items |=
+              ImportItemMaskFromString(item.GetString());
+        }
+      }
+    }
+    return true;
+  }
+
+  bool IsValidSourceIndex(int index) const {
+    return index >= 0 && importer_list_ &&
+           index < static_cast<int>(importer_list_->count());
+  }
+
+  uint16_t GetEffectiveImportItems(
+      const user_data_importer::SourceProfile& source_profile,
+      const ImportRequestSelection& selection) const {
+    uint16_t supported_items =
+        source_profile.services_supported & kBrowserOSImportableItems;
+    return selection.has_selected_items
+               ? (selection.selected_items & supported_items)
+               : supported_items;
+  }
+
+  std::string GetDisplayName(
+      const user_data_importer::SourceProfile& source_profile) const {
+    std::string browser_name = base::UTF16ToUTF8(source_profile.importer_name);
+    std::string profile_name = base::UTF16ToUTF8(source_profile.profile);
+    return profile_name.empty() ? browser_name
+                                : browser_name + " - " + profile_name;
+  }
+
+  base::ListValue BuildSources() const {
+    base::ListValue sources;
+    for (size_t i = 0; importer_list_ && i < importer_list_->count(); ++i) {
+      const user_data_importer::SourceProfile& source_profile =
+          importer_list_->GetSourceProfileAt(i);
+      uint16_t services = source_profile.services_supported;
+      std::string browser_name =
+          base::UTF16ToUTF8(source_profile.importer_name);
+      std::string profile_name = base::UTF16ToUTF8(source_profile.profile);
+
+      base::DictValue source;
+      source.Set("id", SourceIdForIndex(i));
+      source.Set("displayName", GetDisplayName(source_profile));
+      source.Set("browserName", browser_name);
+      source.Set("profileName", profile_name);
+      source.Set("supportedItems", ImportItemsFromMask(services));
+      source.Set("recommendedItems", ImportItemsFromMask(services));
+      sources.Append(std::move(source));
+    }
+    return sources;
+  }
+
+  const char* ImportSourceResultStatusToString(
+      ImportSourceResultStatus status) const {
+    switch (status) {
+      case ImportSourceResultStatus::kPending:
+        return "pending";
+      case ImportSourceResultStatus::kImporting:
+        return "importing";
+      case ImportSourceResultStatus::kSucceeded:
+        return "succeeded";
+      case ImportSourceResultStatus::kFailed:
+        return "failed";
+    }
+    NOTREACHED();
+  }
+
+  base::ListValue BuildResults() const {
+    base::ListValue results;
+    for (const ImportSourceResult& result : import_results_) {
+      base::DictValue result_value;
+      result_value.Set("sourceId", result.source_id);
+      result_value.Set("displayName", result.display_name);
+      result_value.Set("status",
+                       ImportSourceResultStatusToString(result.status));
+      results.Append(std::move(result_value));
+    }
+    return results;
+  }
+
+  base::DictValue BuildProgress() const {
+    base::DictValue progress;
+    progress.Set("completedItems", ImportItemsFromMask(completed_items_));
+    progress.Set("totalItems",
+                 static_cast<int>(ImportItemsFromMask(imported_items_).size()));
+    progress.Set("completedSources", GetCompletedSourceCount());
+    progress.Set("totalSources", static_cast<int>(import_results_.size()));
+    if (has_current_source_) {
+      const ImportQueueEntry& entry = import_queue_[current_source_index_];
+      progress.Set("currentSourceId", entry.source_id);
+      progress.Set("currentSourceName", entry.display_name);
+    }
+    const char* current_item = ImportItemToString(current_item_);
+    if (current_item) {
+      progress.Set("currentItem", current_item);
+    }
+    return progress;
+  }
+
+  void SendState(std::string_view status) {
+    if (IsJavascriptAllowed()) {
+      base::DictValue state;
+      state.Set("apiVersion", kBrowserOSOnboardingApiVersion);
+      state.Set("status", std::string(status));
+      state.Set("sources", BuildSources());
+      if (!import_results_.empty()) {
+        state.Set("results", BuildResults());
+      }
+      if (imported_items_ || !import_results_.empty()) {
+        state.Set("progress", BuildProgress());
+      }
+      CallJavascriptFunction("browserosOnboarding.receiveState", state);
+    }
+  }
+
+  void SendFailure(const std::string& code, const std::string& message) {
+    SendFailure("failed", code, message);
+  }
+
+  void SendFailure(std::string_view status,
+                   const std::string& code,
+                   const std::string& message) {
+    if (IsJavascriptAllowed()) {
+      base::DictValue state;
+      state.Set("apiVersion", kBrowserOSOnboardingApiVersion);
+      state.Set("status", std::string(status));
+      state.Set("sources", BuildSources());
+      if (!import_results_.empty()) {
+        state.Set("results", BuildResults());
+      }
+      if (imported_items_ || !import_results_.empty()) {
+        state.Set("progress", BuildProgress());
+      }
+      base::DictValue error;
+      error.Set("code", code);
+      error.Set("message", message);
+      state.Set("error", std::move(error));
+      CallJavascriptFunction("browserosOnboarding.receiveState", state);
+    }
+  }
+
+  void ImportStarted() override { SendState("importing"); }
+
+  void ImportItemStarted(user_data_importer::ImportItem item) override {
+    current_item_ = item;
+    SendState("importing");
+  }
+
+  void ImportItemEnded(user_data_importer::ImportItem item) override {
+    completed_items_ |= static_cast<uint16_t>(item);
+    current_item_ = user_data_importer::NONE;
+    import_did_succeed_ = true;
+    SendState("importing");
+  }
+
+  void ImportEnded() override {
+    if (importer_host_) {
+      importer_host_->set_observer(nullptr);
+      importer_host_ = nullptr;
+    }
+    if (import_results_.empty()) {
+      return;
+    }
+
+    current_item_ = user_data_importer::NONE;
+    if (import_queue_index_ < import_results_.size()) {
+      import_results_[import_queue_index_].status =
+          import_did_succeed_ ? ImportSourceResultStatus::kSucceeded
+                              : ImportSourceResultStatus::kFailed;
+      ++import_queue_index_;
+    }
+
+    if (IsImportQueueTerminal()) {
+      has_current_source_ = false;
+      SendState(GetTerminalImportStatus());
+      return;
+    }
+
+    SendState("importing");
+    PostStartNextImport();
+  }
+
+  void StartNextImport() {
+    if (import_queue_index_ >= import_queue_.size()) {
+      has_current_source_ = false;
+      SendState(GetTerminalImportStatus());
+      return;
+    }
+
+    current_source_index_ = import_queue_index_;
+    has_current_source_ = true;
+    current_item_ = user_data_importer::NONE;
+    completed_items_ = user_data_importer::NONE;
+    imported_items_ = import_queue_[import_queue_index_].imported_items;
+    import_did_succeed_ = false;
+
+    if (!imported_items_) {
+      import_results_[import_queue_index_].status =
+          ImportSourceResultStatus::kFailed;
+      ++import_queue_index_;
+      bool is_terminal = IsImportQueueTerminal();
+      if (is_terminal) {
+        has_current_source_ = false;
+      }
+      SendState(is_terminal ? GetTerminalImportStatus() : "importing");
+      if (!is_terminal) {
+        PostStartNextImport();
+      }
+      return;
+    }
+
+    import_results_[import_queue_index_].status =
+        ImportSourceResultStatus::kImporting;
+    if (importer_host_) {
+      importer_host_->set_observer(nullptr);
+      importer_host_ = nullptr;
+    }
+    importer_host_ = new ExternalProcessImporterHost();
+    importer_host_->set_observer(this);
+    Profile* profile = Profile::FromWebUI(web_ui());
+    SendState("importing");
+    importer_host_->StartImportSettings(
+        import_queue_[import_queue_index_].source_profile, profile,
+        imported_items_, new ProfileWriter(profile));
+  }
+
+  void PostStartNextImport() {
+    // The host deletes itself after ImportEnded(), so the next host starts
+    // later.
+    base::SequencedTaskRunner::GetCurrentDefault()->PostTask(
+        FROM_HERE, base::BindOnce(&BrowserOSOnboardingHandler::StartNextImport,
+                                  weak_factory_.GetWeakPtr()));
+  }
+
+  bool IsImportQueueRunning() const {
+    return !import_results_.empty() && !IsImportQueueTerminal();
+  }
+
+  bool IsImportQueueTerminal() const {
+    if (import_results_.empty()) {
+      return false;
+    }
+    return GetCompletedSourceCount() ==
+           static_cast<int>(import_results_.size());
+  }
+
+  int GetCompletedSourceCount() const {
+    int count = 0;
+    for (const ImportSourceResult& result : import_results_) {
+      if (result.status == ImportSourceResultStatus::kSucceeded ||
+          result.status == ImportSourceResultStatus::kFailed) {
+        ++count;
+      }
+    }
+    return count;
+  }
+
+  int GetSucceededSourceCount() const {
+    int count = 0;
+    for (const ImportSourceResult& result : import_results_) {
+      if (result.status == ImportSourceResultStatus::kSucceeded) {
+        ++count;
+      }
+    }
+    return count;
+  }
+
+  const char* GetTerminalImportStatus() const {
+    return GetSucceededSourceCount() > 0 ? "succeeded" : "failed";
+  }
+
+  void ResetImportState() {
+    weak_factory_.InvalidateWeakPtrs();
+    current_item_ = user_data_importer::NONE;
+    completed_items_ = user_data_importer::NONE;
+    imported_items_ = user_data_importer::NONE;
+    import_did_succeed_ = false;
+    import_queue_.clear();
+    import_results_.clear();
+    import_queue_index_ = 0;
+    current_source_index_ = 0;
+    has_current_source_ = false;
+  }
+
+  std::unique_ptr<ImporterList> importer_list_;
+  raw_ptr<ExternalProcessImporterHost> importer_host_ = nullptr;
+  base::RepeatingClosure completion_callback_;
+  std::vector<ImportQueueEntry> import_queue_;
+  std::vector<ImportSourceResult> import_results_;
+  user_data_importer::ImportItem current_item_ = user_data_importer::NONE;
+  uint16_t completed_items_ = user_data_importer::NONE;
+  uint16_t imported_items_ = user_data_importer::NONE;
+  size_t import_queue_index_ = 0;
+  size_t current_source_index_ = 0;
+  bool importer_list_loaded_ = false;
+  bool import_did_succeed_ = false;
+  bool has_current_source_ = false;
+  bool completion_handled_ = false;
+  base::WeakPtrFactory<BrowserOSOnboardingHandler> weak_factory_{this};
+};
+
+BrowserOSOnboardingUIConfig::BrowserOSOnboardingUIConfig()
+    : DefaultWebUIConfig(content::kChromeUIScheme,
+                         chrome::kChromeUIBrowserOSOnboardingHost) {}
+
+BrowserOSOnboarding::BrowserOSOnboarding(content::WebUI* web_ui)
+    : content::WebUIController(web_ui) {
+  content::WebUIDataSource* source = content::WebUIDataSource::CreateAndAdd(
+      Profile::FromWebUI(web_ui), chrome::kChromeUIBrowserOSOnboardingHost);
+  webui::SetupWebUIDataSource(source, kBrowserosOnboardingResources,
+                              IDR_BROWSEROS_ONBOARDING_INDEX_HTML);
+
+  auto handler = std::make_unique<BrowserOSOnboardingHandler>();
+  handler_ = handler.get();
+  web_ui->AddMessageHandler(std::move(handler));
+}
+
+BrowserOSOnboarding::~BrowserOSOnboarding() = default;
+
+void BrowserOSOnboarding::SetCompletionCallback(
+    base::RepeatingClosure completion_callback) {
+  if (handler_) {
+    handler_->SetCompletionCallback(std::move(completion_callback));
+  }
+}
+
+WEB_UI_CONTROLLER_TYPE_IMPL(BrowserOSOnboarding)
