import QtQuick
import QtQuick.Controls

// The vertical scroll bar of the lists. The default one is a few pixels wide, in a colour close to
// the background, and fades out unless the list is being scrolled or hovered, so a long list looks
// like it has no scroll bar at all. This one is wider, takes its colour from the text so it
// contrasts in light and dark schemes, and stays visible whenever there is more to see.
ScrollBar {
    id: bar

    policy: size < 1 ? ScrollBar.AlwaysOn : ScrollBar.AlwaysOff
    implicitWidth: 14

    contentItem: Rectangle {
        implicitWidth: 10
        radius: 5
        color: bar.pressed ? bar.palette.highlight : bar.palette.text
        opacity: bar.pressed ? 1 : (bar.hovered ? 0.7 : 0.45)
    }

    background: Rectangle {
        color: bar.palette.text
        opacity: 0.08
    }
}
