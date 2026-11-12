import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Logos.Theme
import Logos.Controls

// The Agent app: connect an MCP client, choose the tool groups the agent may
// use, and see its calls. All state lives in the logos_agents module; the backend
// relays it as JSON PROPs.
Item {
    id: root

    readonly property var backend: logos.module("logos_agents_app")
    property bool ready: false

    readonly property var status: parse(backend ? backend.status : "{}", {})
    readonly property var permissions: parse(backend ? backend.permissions : "{}", {})
    readonly property var activity: parse(backend ? backend.activity : "[]", [])
    readonly property var wallet: parse(backend ? backend.wallet : "{}", {})
    readonly property var approvals: parse(backend ? backend.approvals : "{}", {})
    readonly property var offersState: parse(backend ? backend.offers : "{}", {})
    // The offers being edited. Taken from the module until the user changes one.
    property var offerDraft: []
    property bool offersEdited: false
    onOffersStateChanged: if (!offersEdited) offerDraft = (offersState.offers || []).slice()

    function setOffer(index, key, value) {
        const next = offerDraft.slice()
        next[index] = Object.assign({}, next[index])
        if (value === "" || value === null) delete next[index][key]
        else next[index][key] = value
        offerDraft = next
        offersEdited = true
    }

    function saveOffers() {
        const reply = parse(backend.saveOffers(JSON.stringify(offerDraft)), {})
        if (reply.error) {
            notice = reply.error
            return
        }
        offersEdited = false
        offerDraft = (reply.offers || []).slice()
        notice = qsTr("Offers saved")
    }
    // The account waiting for the user to confirm handing it over.
    property var handOver: null

    property string token: ""
    property bool tokenShown: false
    property string notice: ""
    // Accounts made to be paid for one service job stay folded into one line.
    property bool showJobAccounts: false
    readonly property var jobAccounts: (wallet.accounts || []).filter(function (a) { return a.job })

    readonly property var groups: [
        { key: "messaging", title: qsTr("Messaging"), detail: qsTr("Read and send messages, RLN status") },
        { key: "storage", title: qsTr("Storage"), detail: qsTr("Upload and download files in the files folder") },
        { key: "wallet", title: qsTr("Wallet"), detail: qsTr("See accounts and balances, create agent accounts") },
        { key: "transfers", title: qsTr("Transfers"), detail: qsTr("Send LEZ from the agent accounts, and ask to send TestToken from your chain node") },
        { key: "discovery", title: qsTr("Discovery"), detail: qsTr("Advertise and find services, see connected peers") },
        { key: "chain", title: qsTr("Chain"), detail: qsTr("Read the Logos chain: blocks, transactions and network info") },
        { key: "chat", title: qsTr("Chat"), detail: qsTr("Uses your Basecamp chat account: the agent reads and sends in every Logos Chat conversation you have") },
        { key: "services", title: qsTr("Services"), detail: qsTr("Offer paid services to other agents and hire them. Needs Chat, and paying needs Transfers") },
        { key: "ask_before_transfer", title: qsTr("Ask me before each transfer"), detail: qsTr("Every transfer waits for your OK in the Wallet tab. Off: the agent sends on its own") }
    ]

    function parse(text, fallback) {
        try { return JSON.parse(text) } catch (e) { return fallback }
    }

    function call(pending, done) {
        logos.watch(pending, function (raw) {
            var reply = parse(raw, {})
            if (reply.error) root.notice = reply.error
            else done(reply)
        }, function (error) { root.notice = String(error) })
    }

    function loadToken() {
        call(backend.token(), function (reply) { root.token = reply.token || "" })
    }

    function copy(text) {
        clipboard.text = text
        clipboard.selectAll()
        clipboard.copy()
        root.notice = qsTr("Copied")
    }

    function connectCommand() {
        return "claude mcp add --transport http logos " + (status.url || "")
            + " --header \"Authorization: Bearer " + token + "\""
    }

    // lepta as LEZ (1 LEZ = 10^9 lepta), from the integer string itself: no
    // floating point, so small and very large balances both show exactly.
    // LEZ and the Logos chain's TestToken both count 10^9 lepta to the token.
    function lez(lepta, symbol) {
        if (lepta === null || lepta === undefined || !/^[0-9]+$/.test(String(lepta))) return "?"
        const digits = String(lepta).replace(/^0+(?=.)/, "").padStart(10, "0")
        const whole = digits.slice(0, -9)
        const fraction = digits.slice(-9).replace(/0+$/, "")
        return whole + (fraction ? "." + fraction : "") + " " + (symbol || "LEZ")
    }

    // Lepta as digit strings, added exactly.
    function addLepta(a, b) {
        let sum = "", carry = 0
        a = String(a); b = String(b)
        for (let i = a.length - 1, j = b.length - 1; i >= 0 || j >= 0 || carry; i--, j--) {
            const d = (i >= 0 ? +a[i] : 0) + (j >= 0 ? +b[j] : 0) + carry
            sum = (d % 10) + sum
            carry = d >= 10 ? 1 : 0
        }
        return sum.replace(/^0+(?=.)/, "")
    }

    function time(seconds) {
        return new Date(seconds * 1000).toLocaleTimeString(Qt.locale(), Locale.ShortFormat)
    }

    Connections {
        target: logos
        function onViewModuleReadyChanged(moduleName, isReady) {
            if (moduleName !== "logos_agents_app") return
            root.ready = isReady && root.backend !== null
            if (root.ready) root.loadToken()
        }
    }
    Component.onCompleted: {
        root.ready = root.backend !== null && logos.isViewModuleReady("logos_agents_app")
        if (root.ready) root.loadToken()
    }

    // QML's own clipboard access: the backend runs headless.
    TextEdit { id: clipboard; visible: false }

    Rectangle { anchors.fill: parent; color: Theme.palette.background }

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: Theme.spacing.xlarge
        spacing: Theme.spacing.large

        LogosText {
            text: qsTr("Agent Services")
            font.pixelSize: Theme.typography.titleText
            font.weight: Theme.typography.weightBold
        }

        LogosText {
            Layout.fillWidth: true
            wrapMode: Text.WordWrap
            color: root.status.running ? Theme.palette.success : Theme.palette.warning
            text: !root.ready ? qsTr("Connecting to logos_agents…")
                : root.status.running ? qsTr("Serving MCP at %1").arg(root.status.url)
                : qsTr("Not serving: %1").arg(root.status.error || qsTr("starting"))
        }

        TabBar {
            id: tabs
            Layout.fillWidth: true
            TabButton { text: qsTr("Connect") }
            TabButton { text: qsTr("Permissions") }
            TabButton { text: qsTr("Wallet") }
            TabButton { text: qsTr("Offers") }
            TabButton { text: qsTr("Activity") }
        }

        StackLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            currentIndex: tabs.currentIndex

            // ---- Connect -------------------------------------------------
            ColumnLayout {
                spacing: Theme.spacing.medium

                LogosText {
                    Layout.fillWidth: true
                    wrapMode: Text.WordWrap
                    color: Theme.palette.textSecondary
                    text: qsTr("Point your MCP client at this address with the token below. Anyone with the token can use the tools you allow.")
                }

                LogosText { text: qsTr("Address"); font.weight: Theme.typography.weightMedium }
                RowLayout {
                    LogosText { text: root.status.url || "-"; Layout.fillWidth: true }
                    LogosButton { text: qsTr("Copy"); enabled: !!root.status.url; onClicked: root.copy(root.status.url) }
                }

                LogosText { text: qsTr("Token"); font.weight: Theme.typography.weightMedium }
                RowLayout {
                    LogosText {
                        Layout.fillWidth: true
                        elide: Text.ElideMiddle
                        font.family: "monospace"
                        text: root.token === "" ? "-" : root.tokenShown ? root.token : "•".repeat(24)
                    }
                    LogosButton { text: root.tokenShown ? qsTr("Hide") : qsTr("Show"); onClicked: root.tokenShown = !root.tokenShown }
                    LogosButton { text: qsTr("Copy"); enabled: root.token !== ""; onClicked: root.copy(root.token) }
                }

                LogosText { text: qsTr("Claude Code"); font.weight: Theme.typography.weightMedium }
                RowLayout {
                    LogosText {
                        Layout.fillWidth: true
                        elide: Text.ElideRight
                        font.family: "monospace"
                        color: Theme.palette.textSecondary
                        text: "claude mcp add --transport http logos " + (root.status.url || "") + " --header …"
                    }
                    LogosButton { text: qsTr("Copy command"); enabled: root.token !== ""; onClicked: root.copy(root.connectCommand()) }
                }

                LogosButton {
                    text: qsTr("Issue a new token")
                    onClicked: root.call(root.backend.rotateToken(), function (reply) {
                        root.token = reply.token
                        root.notice = qsTr("New token issued. The old one no longer works.")
                    })
                }

                Item { Layout.fillHeight: true }
            }

            // ---- Permissions ---------------------------------------------
            ColumnLayout {
                spacing: Theme.spacing.medium

                LogosText {
                    Layout.fillWidth: true
                    wrapMode: Text.WordWrap
                    color: Theme.palette.textSecondary
                    text: qsTr("Turned-off tools disappear from the agent's list and are refused if called. Node status is always available.")
                }

                Repeater {
                    model: root.groups
                    delegate: RowLayout {
                        required property var modelData
                        Layout.fillWidth: true
                        ColumnLayout {
                            Layout.fillWidth: true
                            spacing: 2
                            // fillWidth on the labels too: a fixed-size label caps the
                            // row's maximum width, which keeps the switch from the edge.
                            LogosText { Layout.fillWidth: true; text: modelData.title; font.weight: Theme.typography.weightMedium }
                            LogosText { Layout.fillWidth: true; text: modelData.detail; color: Theme.palette.textSecondary; font.pixelSize: Theme.typography.secondaryText }
                        }
                        // Always shows what logos_agents saved: any flip by input snaps back,
                        // and a click only asks for the change.
                        Switch {
                            // Asking is off unless saved on; tool groups are on unless saved off.
                            readonly property bool saved: modelData.key === "ask_before_transfer"
                                ? root.permissions[modelData.key] === true
                                : root.permissions[modelData.key] !== false
                            checked: saved
                            onCheckedChanged: if (checked !== saved) checked = Qt.binding(function () { return saved })
                            enabled: root.ready
                            onClicked: root.call(root.backend.setPermission(modelData.key, !saved), function () {})
                        }
                    }
                }

                Item { Layout.fillHeight: true }
            }

            // ---- Wallet --------------------------------------------------
            ColumnLayout {
                spacing: Theme.spacing.medium

                LogosText {
                    Layout.fillWidth: true
                    wrapMode: Text.WordWrap
                    color: root.wallet.error ? Theme.palette.warning : Theme.palette.textSecondary
                    text: root.wallet.error ? root.wallet.error
                        : qsTr("The agent sends only from its agent accounts. Fund them with what it may spend.")
                }

                LogosText {
                    Layout.fillWidth: true
                    wrapMode: Text.WordWrap
                    visible: !!root.wallet.warning
                    color: Theme.palette.warning
                    text: root.wallet.warning || ""
                }

                RowLayout {
                    visible: !root.wallet.error
                    Layout.fillWidth: true
                    LogosText {
                        Layout.fillWidth: true
                        color: Theme.palette.textTertiary
                        text: root.wallet.status
                            ? qsTr("%1, block %2").arg(root.wallet.status.sequencer).arg(root.wallet.status.block_height || "?")
                            : ""
                    }
                    LogosButton {
                        text: qsTr("New agent account")
                        enabled: root.ready
                        onClicked: root.call(root.backend.newAgentAccount(), function (reply) {
                            root.notice = reply.note || qsTr("New agent account %1").arg(reply.address)
                        })
                    }
                }

                // Transfers the agent asked for, waiting for the user's answer.
                LogosText {
                    visible: (root.approvals.waiting || []).length > 0
                    font.weight: Theme.typography.weightBold
                    color: Theme.palette.warning
                    text: qsTr("Waiting for you")
                }
                Repeater {
                    model: root.approvals.waiting || []
                    delegate: RowLayout {
                        required property var modelData
                        Layout.fillWidth: true
                        LogosText {
                            Layout.fillWidth: true
                            wrapMode: Text.WordWrap
                            // A program call, such as an escrow open, says what it does.
                            // A chain transfer spends the chain node's own wallet.
                            text: modelData.chain
                                ? qsTr("%1 on the Logos chain from your node's %2 to %3, asked at %4")
                                    .arg(root.lez(modelData.amount_lepta, "TestToken")).arg(modelData.from)
                                    .arg(modelData.to).arg(root.time(modelData.requested_at))
                                : modelData.call
                                ? qsTr("%1 from %2 %3, asked at %4")
                                    .arg(root.lez(modelData.amount_lepta)).arg(modelData.from)
                                    .arg(modelData.call.description)
                                    .arg(root.time(modelData.requested_at))
                                : qsTr("%1 from %2 to %3, asked at %4")
                                    .arg(root.lez(modelData.amount_lepta)).arg(modelData.from)
                                    .arg(modelData.to).arg(root.time(modelData.requested_at))
                        }
                        LogosButton {
                            text: qsTr("Deny")
                            enabled: root.ready
                            onClicked: root.call(root.backend.deny(modelData.id), function () {
                                root.notice = qsTr("Denied: the transfer will not be sent.")
                            })
                        }
                        LogosButton {
                            text: qsTr("Approve")
                            enabled: root.ready
                            onClicked: root.call(root.backend.approve(modelData.id), function () {
                                root.notice = qsTr("Approved: sending it now.")
                            })
                        }
                    }
                }

                // Agent accounts this wallet doesn't hold: still granted, and back
                // in force when their wallet opens, so they can be taken back here.
                Repeater {
                    model: root.wallet.missing_agent_accounts || []
                    delegate: RowLayout {
                        required property var modelData
                        Layout.fillWidth: true
                        LogosText {
                            Layout.fillWidth: true
                            elide: Text.ElideMiddle
                            color: Theme.palette.warning
                            text: qsTr("Agent account not in this wallet: %1").arg(modelData)
                        }
                        LogosButton {
                            text: qsTr("Take back")
                            enabled: root.ready
                            onClicked: root.call(root.backend.setAgentAccount(modelData, false), function () {
                                root.notice = qsTr("The agent can no longer send from that account.")
                            })
                        }
                    }
                }

                RowLayout {
                    Layout.fillWidth: true
                    visible: root.jobAccounts.length > 0
                    LogosText {
                        Layout.fillWidth: true
                        wrapMode: Text.WordWrap
                        color: Theme.palette.textSecondary
                        text: qsTr("Job accounts: %1, holding %2. Each service job is paid to its own account, which the agent can spend from.")
                            .arg(root.jobAccounts.length)
                            .arg(root.lez(root.jobAccounts.reduce(function (total, a) {
                                return /^[0-9]+$/.test(a.balance_lepta || "") ? root.addLepta(total, a.balance_lepta) : total
                            }, "0")))
                    }
                    LogosButton {
                        text: root.showJobAccounts ? qsTr("Hide") : qsTr("Show")
                        onClicked: root.showJobAccounts = !root.showJobAccounts
                    }
                }

                ListView {
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    clip: true
                    spacing: Theme.spacing.small
                    model: (root.wallet.accounts || []).filter(function (a) { return root.showJobAccounts || !a.job })
                    delegate: RowLayout {
                        required property var modelData
                        width: ListView.view.width
                        ColumnLayout {
                            Layout.fillWidth: true
                            spacing: 2
                            LogosText {
                                Layout.fillWidth: true
                                elide: Text.ElideMiddle
                                font.family: "monospace"
                                text: modelData.address || modelData.account_id
                            }
                            LogosText {
                                Layout.fillWidth: true
                                color: Theme.palette.textSecondary
                                font.pixelSize: Theme.typography.secondaryText
                                wrapMode: Text.WordWrap
                                text: root.lez(modelData.balance_lepta)
                                    + (modelData["public"] ? "" : qsTr(", private"))
                                    + (modelData.designated ? qsTr(", agent account") : "")
                                    + (modelData.job ? qsTr(", job account for %1").arg(modelData.job.service) : "")
                                    + (modelData.note ? " (" + modelData.note + ")" : "")
                            }
                        }
                        LogosButton {
                            text: qsTr("Copy")
                            onClicked: root.copy(modelData.address || modelData.account_id)
                        }
                        // Transfers send from public accounts only.
                        // Same rule as the permission switches: it shows what was saved,
                        // and a click asks. Handing over needs a confirmation; taking back does not.
                        Switch {
                            readonly property bool saved: modelData.designated === true
                            visible: modelData["public"]
                            checked: saved
                            onCheckedChanged: if (checked !== saved) checked = Qt.binding(function () { return saved })
                            enabled: root.ready
                            onClicked: {
                                if (!saved) {
                                    root.handOver = modelData
                                } else {
                                    root.call(root.backend.setAgentAccount(modelData.account_id, false), function () {
                                        root.notice = qsTr("The agent can no longer send from this account.")
                                    })
                                }
                            }
                        }
                    }
                }
            }

            // ---- Offers --------------------------------------------------
            ColumnLayout {
                spacing: Theme.spacing.medium

                LogosText {
                    Layout.fillWidth: true
                    wrapMode: Text.WordWrap
                    color: Theme.palette.textSecondary
                    text: qsTr("Services this agent offers. An offer with a handler runs on its own: requests are quoted at its price, paid jobs go to the handler's URL, and its answer is delivered. Without a handler, your agent answers the requests.")
                }

                ListView {
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    clip: true
                    spacing: Theme.spacing.medium
                    model: root.offerDraft
                    delegate: ColumnLayout {
                        required property var modelData
                        required property int index
                        width: ListView.view.width
                        spacing: Theme.spacing.small
                        readonly property var applied: (root.offersState.applied || {})[modelData.service_id] || null

                        RowLayout {
                            LogosText {
                                Layout.fillWidth: true
                                font.weight: Theme.typography.weightMedium
                                text: modelData.service_id || qsTr("New offer")
                            }
                            LogosText {
                                visible: !root.offersEdited && applied !== null
                                color: applied && applied.error ? Theme.palette.error : Theme.palette.success
                                text: applied && applied.error ? applied.error : qsTr("advertised")
                                elide: Text.ElideRight
                                Layout.maximumWidth: 360
                            }
                            LogosButton {
                                text: qsTr("Remove")
                                onClicked: {
                                    const next = root.offerDraft.slice()
                                    next.splice(index, 1)
                                    root.offerDraft = next
                                    root.offersEdited = true
                                }
                            }
                        }

                        GridLayout {
                            Layout.fillWidth: true
                            columns: 2
                            columnSpacing: Theme.spacing.medium

                            LogosText { text: qsTr("Service id") }
                            TextField {
                                Layout.fillWidth: true
                                placeholderText: "my-app/summarize/v1"
                                text: modelData.service_id || ""
                                onEditingFinished: if (text !== (modelData.service_id || "")) root.setOffer(index, "service_id", text)
                            }
                            LogosText { text: qsTr("Tag") }
                            TextField {
                                Layout.fillWidth: true
                                placeholderText: qsTr("what buyers see, at most 33 bytes")
                                text: modelData.tag || ""
                                onEditingFinished: if (text !== (modelData.tag || "")) root.setOffer(index, "tag", text)
                            }
                            LogosText { text: qsTr("Price (lepta)") }
                            RowLayout {
                                Layout.fillWidth: true
                                TextField {
                                    Layout.fillWidth: true
                                    validator: RegularExpressionValidator { regularExpression: /[0-9]*/ }
                                    text: modelData.price_lepta || ""
                                    onEditingFinished: if (text !== (modelData.price_lepta || "")) root.setOffer(index, "price_lepta", text)
                                }
                                LogosText { text: modelData.price_lepta ? root.lez(modelData.price_lepta) : ""; color: Theme.palette.textSecondary }
                            }
                            LogosText { text: qsTr("Deliver within (s)") }
                            SpinBox {
                                from: 1; to: 86400; editable: true
                                value: modelData.deliver_within_s || 600
                                onValueModified: root.setOffer(index, "deliver_within_s", value)
                            }
                            LogosText { text: qsTr("Handler URL") }
                            TextField {
                                Layout.fillWidth: true
                                placeholderText: qsTr("http://127.0.0.1:8080/run, or empty for your agent")
                                text: modelData.handler || ""
                                onEditingFinished: if (text !== (modelData.handler || "")) root.setOffer(index, "handler", text)
                            }
                            LogosText { text: qsTr("Jobs at once") }
                            SpinBox {
                                from: 1; to: 100; editable: true
                                enabled: !!modelData.handler
                                value: modelData.max_jobs || 1
                                onValueModified: root.setOffer(index, "max_jobs", value)
                            }
                            LogosText { text: "" }
                            CheckBox {
                                text: qsTr("The handler may be on another machine")
                                enabled: !!modelData.handler
                                checked: !!modelData.remote_handler
                                onToggled: root.setOffer(index, "remote_handler", checked ? true : null)
                            }
                        }
                    }
                }

                RowLayout {
                    LogosButton {
                        text: qsTr("Add offer")
                        onClicked: {
                            root.offerDraft = root.offerDraft.concat([{ deliver_within_s: 600, max_jobs: 1 }])
                            root.offersEdited = true
                        }
                    }
                    LogosButton {
                        text: qsTr("Save")
                        enabled: root.offersEdited
                        onClicked: root.saveOffers()
                    }
                    LogosButton {
                        text: qsTr("Discard changes")
                        visible: root.offersEdited
                        onClicked: {
                            root.offersEdited = false
                            root.offerDraft = (root.offersState.offers || []).slice()
                        }
                    }
                }
            }

            // ---- Activity ------------------------------------------------
            ColumnLayout {
                spacing: Theme.spacing.small

                LogosText {
                    visible: root.activity.length === 0
                    color: Theme.palette.textSecondary
                    text: qsTr("No tool calls yet.")
                }

                ListView {
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    clip: true
                    spacing: Theme.spacing.small
                    model: root.activity.slice().reverse()
                    delegate: ColumnLayout {
                        required property var modelData
                        width: ListView.view.width
                        spacing: 2
                        RowLayout {
                            LogosText { text: root.time(modelData.time); color: Theme.palette.textTertiary }
                            LogosText { text: modelData.tool; font.weight: Theme.typography.weightMedium }
                            LogosText {
                                text: modelData.ok ? qsTr("ok") : qsTr("failed")
                                color: modelData.ok ? Theme.palette.success : Theme.palette.error
                            }
                        }
                        LogosText {
                            Layout.fillWidth: true
                            elide: Text.ElideRight
                            color: Theme.palette.textSecondary
                            font.pixelSize: Theme.typography.secondaryText
                            text: modelData.args + "  →  " + modelData.result
                        }
                    }
                }
            }
        }

        LogosText {
            Layout.fillWidth: true
            wrapMode: Text.WordWrap
            visible: root.notice !== ""
            color: Theme.palette.textSecondary
            text: root.notice
        }
    }

    // Confirmation before handing an account to the agent. Built in-app from
    // Basecamp's own controls (a popup Dialog drew its text invisibly there), and
    // it blocks the view behind it until answered.
    Rectangle {
        anchors.fill: parent
        visible: root.handOver !== null
        color: "#b3000000"
        MouseArea { anchors.fill: parent }

        Rectangle {
            anchors.centerIn: parent
            width: Math.min(parent.width - 2 * Theme.spacing.xlarge, 520)
            height: confirmCard.implicitHeight + 2 * Theme.spacing.xlarge
            radius: Theme.spacing.radiusLarge
            color: Theme.palette.backgroundTertiary
            border.color: Theme.palette.border

            ColumnLayout {
                id: confirmCard
                anchors.fill: parent
                anchors.margins: Theme.spacing.xlarge
                spacing: Theme.spacing.large

                LogosText {
                    Layout.fillWidth: true
                    font.weight: Theme.typography.weightBold
                    text: qsTr("Hand this account to the agent?")
                }
                LogosText {
                    Layout.fillWidth: true
                    wrapMode: Text.WordWrap
                    color: Theme.palette.textSecondary
                    text: root.handOver
                        ? qsTr("The agent will be able to spend everything in %1, now %2, without asking you.")
                            .arg(root.handOver.address).arg(root.lez(root.handOver.balance_lepta))
                        : ""
                }
                RowLayout {
                    Layout.alignment: Qt.AlignRight
                    spacing: Theme.spacing.medium
                    LogosButton {
                        text: qsTr("Cancel")
                        onClicked: root.handOver = null
                    }
                    LogosButton {
                        text: qsTr("Hand over")
                        onClicked: {
                            const account = root.handOver
                            root.handOver = null
                            root.call(root.backend.setAgentAccount(account.account_id, true), function () {
                                root.notice = qsTr("The agent can now send from this account.")
                            })
                        }
                    }
                }
            }
        }
    }
}
