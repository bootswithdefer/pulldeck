#pragma once

#include <QtGui/QGuiApplication>

namespace pulldeck {

// QGuiApplication's setter is a static member function, which cxx cannot bind directly.
inline void setQuitOnLastWindowClosed(bool quit)
{
    QGuiApplication::setQuitOnLastWindowClosed(quit);
}

} // namespace pulldeck
