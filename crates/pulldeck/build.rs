use cxx_qt_build::{CxxQtBuilder, QmlModule};

fn main() {
    CxxQtBuilder::new_qml_module(
        QmlModule::new("io.github.bootswithdefer.pulldeck")
            // PrBackend is a QAbstractListModel, which QML tooling finds in this module.
            .depend("QtQml.Models")
            .qml_file("qml/main.qml")
            .qml_file("qml/Badge.qml")
            .qml_file("qml/InboxView.qml")
            .qml_file("qml/SignInDialog.qml"),
    )
    .files(["src/backend.rs"])
    .build();
}
