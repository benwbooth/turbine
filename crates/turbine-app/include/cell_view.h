// Turbine's screen in the desktop app: the cell grid, painted with QPainter
// in the desktop's own monospace font (native FreeType text). Box-drawing
// and block characters are drawn as shapes on the cell grid, so frames join
// at any line height, as in Tessera.
#pragma once

#include <QtCore/QTimer>
#include <QtGui/QFont>
#include <QtQuick/QQuickPaintedItem>

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
    void wheelEvent(QWheelEvent* e) override;
    void geometryChange(const QRectF& now, const QRectF& old) override;
    bool event(QEvent* e) override;

private:
    void tick();
    void relayout();
    void afterInput();
    QPoint cellAt(QPointF pos) const;
    bool drawShape(QPainter* p, char32_t c, const QRectF& r, const QColor& fg, const QColor& bg);

    QFont m_font, m_bold, m_italic, m_boldItalic;
    qreal m_cw = 8, m_ch = 16, m_ascent = 12;
    int m_cols = 0, m_rows = 0;
    QTimer m_timer;
    QString m_title, m_menusJson;
    int m_buttons = 0;
    QPoint m_lastCell { -1, -1 };
    qreal m_wheel = 0;
};
