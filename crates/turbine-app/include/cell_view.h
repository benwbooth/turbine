// Turbine's screen in the desktop app: the cell grid, painted with QPainter
// in the desktop's own monospace font (native FreeType text). Box-drawing
// and block characters are drawn as shapes on the cell grid, so frames join
// at any line height, as in Tessera.
#pragma once

#include <QtCore/QTimer>
#include <memory>
#include <mutex>
#include <QtGui/QFont>
#include <QtGui/QImage>
#include <QtCore/QHash>
#include <QtCore/QSet>
#include <QtQuick/QQuickPaintedItem>

namespace turbine { struct Frame; }

class CellView : public QQuickPaintedItem {
    Q_OBJECT
    Q_PROPERTY(QString title READ title NOTIFY titleChanged)
    Q_PROPERTY(QString menusJson READ menusJson NOTIFY menusChanged)
    Q_PROPERTY(qreal cellWidth READ cellWidth NOTIFY metricsChanged)
    Q_PROPERTY(qreal cellHeight READ cellHeight NOTIFY metricsChanged)
public:
    explicit CellView(QQuickItem* parent = nullptr);
    void paint(QPainter* p) override;

    QString title() const { return m_title; }
    QString menusJson() const { return m_menusJson; }
    qreal cellWidth() const { return m_cw; }
    qreal cellHeight() const { return m_ch; }

    Q_INVOKABLE void command(const QString& cmd);

signals:
    void titleChanged();
    void menusChanged();
    void metricsChanged();

protected:
    void keyPressEvent(QKeyEvent* e) override;
    void mousePressEvent(QMouseEvent* e) override;
    void mouseReleaseEvent(QMouseEvent* e) override;
    void mouseMoveEvent(QMouseEvent* e) override;
    void mouseDoubleClickEvent(QMouseEvent* e) override;
    void hoverMoveEvent(QHoverEvent* e) override;
    void hoverLeaveEvent(QHoverEvent* e) override;
    void wheelEvent(QWheelEvent* e) override;
    void geometryChange(const QRectF& now, const QRectF& old) override;
    bool event(QEvent* e) override;

private:
    void tick();
    void relayout();
    void afterInput();
    QPoint cellAt(QPointF pos) const;
    bool drawShape(QPainter* p, char32_t c, const QRectF& r, const QColor& fg, const QColor& bg);
    void brailleDots(QPainter* p, const QRectF& r, char32_t c, const QColor& fg);
    void paintVga(QPainter* p);

    // The frame to paint, taken on the GUI thread: Turbine's state lives
    // there, and paint() runs on Qt Quick's render thread on a real display.
    std::shared_ptr<turbine::Frame> m_frame;
    QFont m_font, m_bold, m_italic, m_boldItalic;
    qreal m_cw = 8, m_ch = 16, m_ascent = 12;
    // VGA mode (the default with the DOS font installed): the screen is
    // drawn at text mode's own pixels, 8 or 9 dots by 16 per character
    // (each dot m_k texture pixels), and the texture is stretched to the
    // cells as a 4:3 monitor did: dots 1.35 times taller than wide.
    bool m_vga = false;
    // Pictures (an image viewer's), loaded on a thread of their own.
    QHash<QString, QImage> m_pictures;
    QSet<QString> m_loading;
    std::mutex m_picMutex;
    void drawPictures(QPainter* p);
    int m_k = 2, m_dots = 9;
    QFont m_vga9, m_vga8;
    QSize m_vgaSize;
    QImage m_vgaImage;
    int m_cols = 0, m_rows = 0;
    QTimer m_timer;
    // The cursor's blink: shown or not, switched every half second (and
    // shown again at once on a key).
    QTimer m_blink;
    bool m_cursorOn = true;
    QString m_title, m_menusJson;
    int m_buttons = 0;
    QPoint m_lastCell { -1, -1 };
    qreal m_wheel = 0;
};
