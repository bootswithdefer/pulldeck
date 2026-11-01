pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import io.github.bootswithdefer.pulldeck

// Signs in to a pulldeck server with GitHub's device flow: show a code, the user enters it on
// GitHub, and the backend finishes the sign-in on its own.
Dialog {
    id: dialog

    required property PrBackend deck
    readonly property bool waiting: deck.auth === "signingIn"

    title: qsTr("Sign in")
    modal: true
    anchors.centerIn: parent
    width: Math.min(parent.width - 48, 460)
    // Cancelling is the only way out of a pending sign-in, not clicking away.
    closePolicy: waiting ? Popup.NoAutoClose : (Popup.CloseOnEscape | Popup.CloseOnPressOutside)

    // Closes itself when the sign-in works.
    Connections {
        target: dialog.deck

        function onAuthChanged(): void {
            if (dialog.deck.auth === "signedIn")
                dialog.close();
        }
    }

    onOpened: server.forceActiveFocus()
    onClosed: if (waiting)
        deck.cancelSignIn()

    contentItem: ColumnLayout {
        spacing: 12

        // Before the user has started.
        ColumnLayout {
            visible: !dialog.waiting
            spacing: 8
            Layout.fillWidth: true

            Label {
                text: qsTr("Server")
                font.bold: true
            }
            TextField {
                id: server
                Layout.fillWidth: true
                text: dialog.deck.serverUrl
                placeholderText: "https://pulldeck.example.com"
                inputMethodHints: Qt.ImhUrlCharactersOnly
                onAccepted: signInButton.clicked()
            }
            Label {
                visible: dialog.deck.authError !== ""
                text: dialog.deck.authError
                color: palette.link
                wrapMode: Text.Wrap
                Layout.fillWidth: true
            }
            Label {
                text: qsTr("You will approve pulldeck on GitHub. The app never sees your GitHub token.")
                wrapMode: Text.Wrap
                opacity: 0.7
                Layout.fillWidth: true
            }
        }

        // Waiting for the user to approve on GitHub.
        ColumnLayout {
            visible: dialog.waiting
            spacing: 8
            Layout.fillWidth: true

            Label {
                text: dialog.deck.userCode === "" ? qsTr("Contacting the server…") : qsTr("Enter this code on GitHub:")
            }
            Label {
                visible: dialog.deck.userCode !== ""
                text: dialog.deck.userCode
                font.pointSize: Application.font.pointSize * 2
                font.bold: true
                font.family: "monospace"
                Layout.alignment: Qt.AlignHCenter
            }
            Button {
                visible: dialog.deck.verificationUri !== ""
                text: qsTr("Open %1").arg(dialog.deck.verificationUri)
                Layout.alignment: Qt.AlignHCenter
                onClicked: Qt.openUrlExternally(dialog.deck.verificationUri)
            }
            BusyIndicator {
                running: dialog.waiting
                Layout.alignment: Qt.AlignHCenter
            }
        }
    }

    footer: DialogButtonBox {
        Button {
            text: qsTr("Cancel")
            DialogButtonBox.buttonRole: DialogButtonBox.RejectRole
            onClicked: {
                dialog.deck.cancelSignIn();
                dialog.close();
            }
        }
        Button {
            id: signInButton
            visible: !dialog.waiting
            text: qsTr("Sign in")
            highlighted: true
            enabled: server.text.trim() !== ""
            DialogButtonBox.buttonRole: DialogButtonBox.ActionRole
            onClicked: dialog.deck.signIn(server.text.trim())
        }
    }
}
