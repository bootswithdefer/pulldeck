pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import io.github.bootswithdefer.pulldeck

// Asks the server to do something about a pull request on someone's behalf: request a review,
// nudge them, or send them a message. None of these touch GitHub.
Dialog {
    id: dialog

    required property PrBackend deck

    // "review", "nudge" or "message".
    property string action: "nudge"
    property string repo: ""
    property int number: 0
    // The request that is in flight, so the outcome of some other action is ignored.
    property string pending: ""
    property string failure: ""

    readonly property bool needsText: action !== "review"
    readonly property string heading: action === "review" ? qsTr("Request a review") : action === "nudge" ? qsTr("Nudge") : qsTr("Send a message")

    function show(kind: string, prRepo: string, prNumber: int): void {
        action = kind;
        repo = prRepo;
        number = prNumber;
        pending = "";
        failure = "";
        to.text = "";
        detail.text = "";
        open();
    }

    function send(): void {
        const login = to.text.trim().replace(/^@/, "");
        if (login === "")
            return;
        failure = "";
        // An action's outcome arrives as a signal; the first one after sending is ours.
        pending = "sent";
        if (action === "review")
            deck.requestReview(repo, number, login);
        else if (action === "nudge")
            deck.nudge(repo, number, login, detail.text.trim());
        else
            deck.sendMessage(repo, number, login, detail.text.trim());
    }

    title: heading
    modal: true
    anchors.centerIn: parent
    width: Math.min(parent.width - 48, 460)
    onOpened: to.forceActiveFocus()

    Connections {
        target: dialog.deck

        function onActionResult(requestId: string, ok: bool, message: string): void {
            if (dialog.pending === "")
                return;
            dialog.pending = "";
            if (ok)
                dialog.close();
            else
                dialog.failure = message;
        }
    }

    contentItem: ColumnLayout {
        spacing: 8

        Label {
            text: dialog.repo + " #" + dialog.number
            opacity: 0.7
        }
        Label {
            text: qsTr("GitHub username")
            font.bold: true
        }
        TextField {
            id: to
            Layout.fillWidth: true
            placeholderText: "octocat"
            onAccepted: dialog.needsText ? detail.forceActiveFocus() : dialog.send()
        }
        Label {
            visible: dialog.needsText
            text: dialog.action === "nudge" ? qsTr("Note (optional)") : qsTr("Message")
            font.bold: true
        }
        TextField {
            id: detail
            visible: dialog.needsText
            Layout.fillWidth: true
            maximumLength: dialog.action === "nudge" ? 500 : 2000
            onAccepted: dialog.send()
        }
        Label {
            visible: dialog.failure !== ""
            text: dialog.failure
            color: palette.link
            wrapMode: Text.Wrap
            Layout.fillWidth: true
        }
    }

    footer: DialogButtonBox {
        Button {
            text: qsTr("Cancel")
            DialogButtonBox.buttonRole: DialogButtonBox.RejectRole
            onClicked: dialog.close()
        }
        Button {
            text: qsTr("Send")
            highlighted: true
            enabled: dialog.pending === "" && to.text.trim() !== "" && (dialog.action !== "message" || detail.text.trim() !== "")
            DialogButtonBox.buttonRole: DialogButtonBox.ActionRole
            onClicked: dialog.send()
        }
    }
}
