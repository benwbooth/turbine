#include "cell_view.h"

#include "turbine-app/src/bridge.cxxqt.h"

#include <QtCore/QCoreApplication>
#include <QtGui/QClipboard>
#include <QtGui/QFontDatabase>
#include <QtGui/QFontMetricsF>
#include <QtGui/QGuiApplication>
#include <QtGui/QKeyEvent>
#include <QtGui/QPainter>
#include <QtQuick/QQuickWindow>

#include <cmath>

namespace {

QColor rgb(uint32_t c) { return QColor((c >> 16) & 0xff, (c >> 8) & 0xff, c & 0xff); }

// The desktop's monospace font, or TURBINE_FONT ("Family 12" or "Family").
QFont baseFont()
{
    QFont f = QFontDatabase::systemFont(QFontDatabase::FixedFont);
    const QString env = qEnvironmentVariable("TURBINE_FONT").trimmed();
    if (!env.isEmpty()) {
        const int sp = env.lastIndexOf(QLatin1Char(' '));
        bool ok = false;
        const double size = sp > 0 ? env.mid(sp + 1).toDouble(&ok) : 0;
        f.setFamily(ok ? env.left(sp) : env);
        if (ok && size > 0) f.setPointSizeF(size);
    }
    f.setStyleHint(QFont::Monospace);
    f.setFixedPitch(true);
    f.setKerning(false);
    return f;
}

// Box drawing: the four arms of a character (up, right, down, left), each
// 0 none, 1 light, 2 heavy, 3 double. Characters not listed fall back to the font.
struct Arms { uint8_t u, r, d, l; };

bool armsOf(char32_t c, Arms& a)
{
    switch (c) {
    case 0x2500: a = {0, 1, 0, 1}; return true; // ─
    case 0x2501: a = {0, 2, 0, 2}; return true; // ━
    case 0x2502: a = {1, 0, 1, 0}; return true; // │
    case 0x2503: a = {2, 0, 2, 0}; return true; // ┃
    case 0x250C: a = {0, 1, 1, 0}; return true; // ┌
    case 0x250F: a = {0, 2, 2, 0}; return true; // ┏
    case 0x2510: a = {0, 0, 1, 1}; return true; // ┐
    case 0x2513: a = {0, 0, 2, 2}; return true; // ┓
    case 0x2514: a = {1, 1, 0, 0}; return true; // └
    case 0x2517: a = {2, 2, 0, 0}; return true; // ┗
    case 0x2518: a = {1, 0, 0, 1}; return true; // ┘
    case 0x251B: a = {2, 0, 0, 2}; return true; // ┛
    case 0x251C: a = {1, 1, 1, 0}; return true; // ├
    case 0x2524: a = {1, 0, 1, 1}; return true; // ┤
    case 0x252C: a = {0, 1, 1, 1}; return true; // ┬
    case 0x2534: a = {1, 1, 0, 1}; return true; // ┴
    case 0x253C: a = {1, 1, 1, 1}; return true; // ┼
    case 0x2550: a = {0, 3, 0, 3}; return true; // ═
    case 0x2551: a = {3, 0, 3, 0}; return true; // ║
    case 0x2552: a = {0, 3, 1, 0}; return true; // ╒
    case 0x2553: a = {0, 1, 3, 0}; return true; // ╓
    case 0x2554: a = {0, 3, 3, 0}; return true; // ╔
    case 0x2555: a = {0, 0, 1, 3}; return true; // ╕
    case 0x2556: a = {0, 0, 3, 1}; return true; // ╖
    case 0x2557: a = {0, 0, 3, 3}; return true; // ╗
    case 0x2558: a = {1, 3, 0, 0}; return true; // ╘
    case 0x2559: a = {3, 1, 0, 0}; return true; // ╙
    case 0x255A: a = {3, 3, 0, 0}; return true; // ╚
    case 0x255B: a = {1, 0, 0, 3}; return true; // ╛
    case 0x255C: a = {3, 0, 0, 1}; return true; // ╜
    case 0x255D: a = {3, 0, 0, 3}; return true; // ╝
    case 0x255E: a = {1, 3, 1, 0}; return true; // ╞
    case 0x255F: a = {3, 1, 3, 0}; return true; // ╟
    case 0x2560: a = {3, 3, 3, 0}; return true; // ╠
    case 0x2561: a = {1, 0, 1, 3}; return true; // ╡
    case 0x2562: a = {3, 0, 3, 1}; return true; // ╢
    case 0x2563: a = {3, 0, 3, 3}; return true; // ╣
    case 0x2564: a = {0, 3, 1, 3}; return true; // ╤
    case 0x2565: a = {0, 1, 3, 1}; return true; // ╥
    case 0x2566: a = {0, 3, 3, 3}; return true; // ╦
    case 0x2567: a = {1, 3, 0, 3}; return true; // ╧
    case 0x2568: a = {3, 1, 0, 1}; return true; // ╨
    case 0x2569: a = {3, 3, 0, 3}; return true; // ╩
    case 0x256A: a = {1, 3, 1, 3}; return true; // ╪
    case 0x256B: a = {3, 1, 3, 1}; return true; // ╫
    case 0x256C: a = {3, 3, 3, 3}; return true; // ╬
    default: return false;
    }
}

} // namespace

CellView::CellView(QQuickItem* parent)
    : QQuickPaintedItem(parent)
{
    setFlag(ItemHasContents);
    setFlag(ItemIsFocusScope);
    setFocus(true);
    setAcceptedMouseButtons(Qt::AllButtons);
    setAcceptHoverEvents(true);
    setAntialiasing(false);
    setOpaquePainting(true);
    setFillColor(Qt::black);

    m_font = baseFont();
    m_bold = m_font;
    m_bold.setBold(true);
    m_italic = m_font;
    m_italic.setItalic(true);
    m_boldItalic = m_bold;
    m_boldItalic.setItalic(true);
    QFontMetricsF fm(m_font);
    // Whole pixels per cell, so lines and glyphs land on the same grid.
    m_cw = std::ceil(fm.horizontalAdvance(QLatin1Char('M')));
    m_ch = std::ceil(fm.lineSpacing());
    m_ascent = fm.ascent() + (m_ch - fm.lineSpacing()) / 2;

    m_timer.setSingleShot(true);
    connect(&m_timer, &QTimer::timeout, this, &CellView::tick);
    m_timer.start(0);
    m_menusJson = QString::fromStdString(std::string(turbine::app_menus_json()));

    // For tests: TURBINE_SHOT=file.png saves the window after
    // TURBINE_SHOT_DELAY ms (default 1500) and quits (run it offscreen).
    const QString shot = qEnvironmentVariable("TURBINE_SHOT");
    if (!shot.isEmpty()) {
        const int delay = qEnvironmentVariableIntValue("TURBINE_SHOT_DELAY");
        QTimer::singleShot(delay > 0 ? delay : 1500, this, [this, shot] {
            if (QQuickWindow* w = window()) w->grabWindow().save(shot);
            QCoreApplication::quit();
        });
    }
}

void CellView::command(const QString& cmd)
{
    turbine::app_command(rust::Str(cmd.toStdString()));
    afterInput();
}

void CellView::relayout()
{
    const int cols = std::max(20, int(width() / m_cw));
    const int rows = std::max(8, int(height() / m_ch));
    if (cols != m_cols || rows != m_rows) {
        m_cols = cols;
        m_rows = rows;
        turbine::app_resize(cols, rows);
        afterInput();
    }
}

void CellView::geometryChange(const QRectF& now, const QRectF& old)
{
    QQuickPaintedItem::geometryChange(now, old);
    relayout();
}

void CellView::tick()
{
    const int next = turbine::app_tick();
    if (next < 0) {
        QCoreApplication::quit();
        return;
    }
    const QString copied = QString::fromStdString(std::string(turbine::app_take_clipboard()));
    if (!copied.isEmpty()) QGuiApplication::clipboard()->setText(copied);
    const QString t = QString::fromStdString(std::string(turbine::app_title()));
    if (t != m_title) {
        m_title = t;
        emit titleChanged();
    }
    if (turbine::app_dirty()) {
        m_frame = std::make_shared<turbine::Frame>(turbine::app_frame());
        update();
        // Check marks and enabled items may have changed.
        const QString menus = QString::fromStdString(std::string(turbine::app_menus_json()));
        if (menus != m_menusJson) {
            m_menusJson = menus;
            emit menusChanged();
        }
    }
    m_timer.start(next);
}

void CellView::afterInput()
{
    // Run a tick soon (coalescing a burst of input into one paint).
    if (!m_timer.isActive() || m_timer.remainingTime() > 1) m_timer.start(0);
}

QPoint CellView::cellAt(QPointF pos) const
{
    return QPoint(int(std::floor(pos.x() / m_cw)), int(std::floor(pos.y() / m_ch)));
}

bool CellView::event(QEvent* e)
{
    // Tab and Shift+Tab are Turbine's (dialogs, editors), not focus moves.
    if (e->type() == QEvent::KeyPress) {
        auto* k = static_cast<QKeyEvent*>(e);
        if (k->key() == Qt::Key_Tab || k->key() == Qt::Key_Backtab) {
            keyPressEvent(k);
            return true;
        }
    }
    return QQuickPaintedItem::event(e);
}

void CellView::keyPressEvent(QKeyEvent* e)
{
    const auto mods = e->modifiers();
    // Before a key that may paste, Turbine learns what the desktop copied.
    if ((mods & Qt::ControlModifier) || (e->key() == Qt::Key_Insert && (mods & Qt::ShiftModifier)))
        turbine::app_set_clipboard(rust::Str(QGuiApplication::clipboard()->text().toStdString()));
    const std::string text = e->text().toStdString();
    if (turbine::app_key(e->key(), uint32_t(mods), rust::Str(text))) {
        e->accept();
        afterInput();
    } else {
        e->ignore();
    }
}

static int buttonOf(Qt::MouseButton b)
{
    switch (b) {
    case Qt::RightButton: return 2;
    case Qt::MiddleButton: return 3;
    default: return 1;
    }
}

void CellView::mousePressEvent(QMouseEvent* e)
{
    forceActiveFocus();
    const QPoint c = cellAt(e->position());
    m_buttons |= e->button();
    if (e->button() == Qt::MiddleButton)
        turbine::app_set_clipboard(rust::Str(QGuiApplication::clipboard()->text(QClipboard::Selection).toStdString()));
    turbine::app_mouse(0, buttonOf(e->button()), c.x(), c.y(), uint32_t(e->modifiers()));
    m_lastCell = c;
    e->accept();
    afterInput();
}

void CellView::mouseDoubleClickEvent(QMouseEvent* e)
{
    // Turbine counts clicks itself.
    mousePressEvent(e);
}

void CellView::mouseReleaseEvent(QMouseEvent* e)
{
    const QPoint c = cellAt(e->position());
    m_buttons &= ~e->button();
    turbine::app_mouse(1, buttonOf(e->button()), c.x(), c.y(), uint32_t(e->modifiers()));
    e->accept();
    afterInput();
}

void CellView::mouseMoveEvent(QMouseEvent* e)
{
    const QPoint c = cellAt(e->position());
    if (c == m_lastCell) return;
    m_lastCell = c;
    const int b = (e->buttons() & Qt::LeftButton) ? 1 : (e->buttons() & Qt::RightButton) ? 2 : 3;
    turbine::app_mouse(2, b, c.x(), c.y(), uint32_t(e->modifiers()));
    afterInput();
}

void CellView::hoverMoveEvent(QHoverEvent* e)
{
    const QPoint c = cellAt(e->position());
    if (c == m_lastCell) return;
    m_lastCell = c;
    turbine::app_mouse(3, 0, c.x(), c.y(), uint32_t(e->modifiers()));
    afterInput();
}

void CellView::wheelEvent(QWheelEvent* e)
{
    // Trackpads send pixels, wheels send angles: one step per 120 or per cell height.
    const QPoint px = e->pixelDelta();
    m_wheel += px.isNull() ? e->angleDelta().y() / 120.0 : px.y() / m_ch;
    const QPoint c = cellAt(e->position());
    while (std::abs(m_wheel) >= 1) {
        const bool up = m_wheel > 0;
        turbine::app_mouse(up ? 4 : 5, 0, c.x(), c.y(), uint32_t(e->modifiers()));
        m_wheel += up ? -1 : 1;
    }
    e->accept();
    afterInput();
}

bool CellView::drawShape(QPainter* p, char32_t c, const QRectF& r, const QColor& fg, const QColor& bg)
{
    // Blocks and shades.
    switch (c) {
    case 0x2588: p->fillRect(r, fg); return true;                                                  // █
    case 0x2580: p->fillRect(QRectF(r.x(), r.y(), r.width(), std::round(r.height() / 2)), fg); return true; // ▀
    case 0x2584: { const qreal h = std::round(r.height() / 2); p->fillRect(QRectF(r.x(), r.bottom() - h, r.width(), h), fg); return true; } // ▄
    case 0x258C: p->fillRect(QRectF(r.x(), r.y(), std::round(r.width() / 2), r.height()), fg); return true; // ▌
    case 0x2590: { const qreal w = std::round(r.width() / 2); p->fillRect(QRectF(r.right() - w, r.y(), w, r.height()), fg); return true; } // ▐
    case 0x2591: case 0x2592: case 0x2593: {                                                         // ░ ▒ ▓
        const qreal a = c == 0x2591 ? 0.25 : c == 0x2592 ? 0.5 : 0.75;
        QColor m(int(fg.red() * a + bg.red() * (1 - a)), int(fg.green() * a + bg.green() * (1 - a)), int(fg.blue() * a + bg.blue() * (1 - a)));
        p->fillRect(r, m);
        return true;
    }
    default: break;
    }
    Arms a;
    if (!armsOf(c, a)) return false;
    const qreal t = std::max<qreal>(1, std::round(m_ch / 16));     // light line
    const qreal T = std::max<qreal>(t + 1, std::round(m_ch / 8));  // heavy line
    const qreal g = std::max<qreal>(t, std::round(m_cw / 5));      // half the gap of a double line
    const qreal cx = std::round(r.x() + r.width() / 2);
    const qreal cy = std::round(r.y() + r.height() / 2);
    const bool anyDouble = a.u == 3 || a.r == 3 || a.d == 3 || a.l == 3;
    auto hline = [&](qreal x0, qreal x1, qreal y, qreal w) { p->fillRect(QRectF(std::min(x0, x1), y - std::floor(w / 2), std::abs(x1 - x0), w), fg); };
    auto vline = [&](qreal y0, qreal y1, qreal x, qreal w) { p->fillRect(QRectF(x - std::floor(w / 2), std::min(y0, y1), w, std::abs(y1 - y0)), fg); };
    const qreal ext = anyDouble ? g : 0;  // single arms reach the double lines
    // Right
    if (a.r == 3) {
        hline(a.u ? cx + g : cx - g, r.right(), cy - g, t);
        hline(a.d ? cx + g : cx - g, r.right(), cy + g, t);
    } else if (a.r) hline(cx - (a.r == 2 ? T : t) / 2 - ext, r.right(), cy, a.r == 2 ? T : t);
    // Left
    if (a.l == 3) {
        hline(r.left(), a.u ? cx - g : cx + g, cy - g, t);
        hline(r.left(), a.d ? cx - g : cx + g, cy + g, t);
    } else if (a.l) hline(r.left(), cx + (a.l == 2 ? T : t) / 2 + ext, cy, a.l == 2 ? T : t);
    // Down
    if (a.d == 3) {
        vline(a.l ? cy + g : cy - g, r.bottom(), cx - g, t);
        vline(a.r ? cy + g : cy - g, r.bottom(), cx + g, t);
    } else if (a.d) vline(cy - (a.d == 2 ? T : t) / 2 - ext, r.bottom(), cx, a.d == 2 ? T : t);
    // Up
    if (a.u == 3) {
        vline(r.top(), a.l ? cy - g : cy + g, cx - g, t);
        vline(r.top(), a.r ? cy - g : cy + g, cx + g, t);
    } else if (a.u) vline(r.top(), cy + (a.u == 2 ? T : t) / 2 + ext, cx, a.u == 2 ? T : t);
    return true;
}

void CellView::paint(QPainter* p)
{
    // (The render thread: only the frame taken on the GUI thread, never Turbine itself.)
    if (!m_frame) return;
    const turbine::Frame& f = *m_frame;
    const QRectF all = boundingRect();
    p->setRenderHint(QPainter::Antialiasing, false);
    p->setRenderHint(QPainter::TextAntialiasing, true);
    const int cols = f.cols, rows = f.rows;
    // The strip right of and below the grid: the desktop pattern's colour.
    if (!f.cells.empty()) p->fillRect(all, rgb(f.cells[0].bg));

    auto cellRect = [&](int x, int y, int w) { return QRectF(x * m_cw, y * m_ch, w * m_cw, m_ch); };
    // Backgrounds first, as runs.
    for (int y = 0; y < rows; ++y) {
        int x = 0;
        while (x < cols) {
            const auto& c = f.cells[size_t(y * cols + x)];
            const bool rev = c.flags & 8;
            const uint32_t bg = rev ? c.fg : c.bg;
            int x1 = x + 1;
            while (x1 < cols) {
                const auto& n = f.cells[size_t(y * cols + x1)];
                if (((n.flags & 8) ? n.fg : n.bg) != bg) break;
                ++x1;
            }
            p->fillRect(cellRect(x, y, x1 - x), rgb(bg));
            x = x1;
        }
    }
    // Then text, shapes and underlines. (The 0.001 px nudge keeps round
    // letters from landing exactly on half pixels at fractional scales.)
    p->save();
    p->translate(0.001, 0.001);
    for (int y = 0; y < rows; ++y) {
        int x = 0;
        while (x < cols) {
            const auto& c = f.cells[size_t(y * cols + x)];
            if (c.flags & 32) { ++x; continue; }
            const bool rev = c.flags & 8;
            const QColor fg = rgb(rev ? c.bg : c.fg), bg = rgb(rev ? c.fg : c.bg);
            const int w = (c.flags & 16) ? 2 : 1;
            const QRectF r = cellRect(x, y, w);
            const QString s = QString::fromUtf8(c.text.data(), int(c.text.size()));
            const char32_t ch = s.isEmpty() ? U' ' : s.toUcs4().value(0);
            if (ch != U' ' && !drawShape(p, ch, r, fg, bg)) {
                // Plain ASCII with the same style: one run (the font is
                // monospace and cells are its advance, so glyphs stay on cells).
                QString run = s;
                int x1 = x + 1;
                if (s.size() == 1 && s[0].unicode() < 0x80 && w == 1) {
                    while (x1 < cols) {
                        const auto& n = f.cells[size_t(y * cols + x1)];
                        if (n.fg != c.fg || n.bg != c.bg || n.flags != c.flags || n.text.size() != 1 || uint8_t(n.text.data()[0]) >= 0x80) break;
                        const char nc = n.text.data()[0];
                        if (nc == ' ' && x1 + 1 < cols && f.cells[size_t(y * cols + x1 + 1)].text.size() != 1) break;
                        run += QLatin1Char(nc);
                        ++x1;
                    }
                }
                const bool bold = c.flags & 1, italic = c.flags & 2;
                p->setFont(bold && italic ? m_boldItalic : bold ? m_bold : italic ? m_italic : m_font);
                p->setPen(fg);
                p->drawText(QPointF(x * m_cw, y * m_ch + m_ascent), run);
                if (c.flags & 4) p->fillRect(QRectF(x * m_cw, y * m_ch + m_ch - std::max<qreal>(1, std::round(m_ch / 16)), (x1 - x) * m_cw, std::max<qreal>(1, std::round(m_ch / 16))), fg);
                x = x1;
                continue;
            }
            if (c.flags & 4) p->fillRect(QRectF(r.x(), r.bottom() - std::max<qreal>(1, std::round(m_ch / 16)), r.width(), std::max<qreal>(1, std::round(m_ch / 16))), fg);
            x += w;
        }
    }
    p->restore();
    // The cursor.
    if (f.cursor_shape >= 0 && f.cursor_x < cols && f.cursor_y < rows && hasActiveFocus()) {
        const auto& c = f.cells[size_t(f.cursor_y * cols + f.cursor_x)];
        const QColor fg = rgb(c.fg);
        const QRectF r = cellRect(f.cursor_x, f.cursor_y, (c.flags & 16) ? 2 : 1);
        if (f.cursor_shape == 0) p->fillRect(QRectF(r.x(), r.bottom() - std::max<qreal>(2, std::round(m_ch / 8)), r.width(), std::max<qreal>(2, std::round(m_ch / 8))), fg);
        else if (f.cursor_shape == 2) p->fillRect(QRectF(r.x(), r.y(), std::max<qreal>(2, std::round(m_cw / 6)), r.height()), fg);
        else {
            p->fillRect(r, fg);
            const QString s = QString::fromUtf8(c.text.data(), int(c.text.size()));
            p->setFont(m_font);
            p->setPen(rgb(c.bg));
            p->drawText(QPointF(r.x() + 0.001, r.y() + m_ascent + 0.001), s);
        }
    }
}
