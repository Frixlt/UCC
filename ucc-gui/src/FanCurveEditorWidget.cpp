#include "FanCurveEditorWidget.hpp"
#include <QPainter>
#include <QPalette>
#include <QMouseEvent>
#include <QResizeEvent>
#include <QEvent>
#include <QMenu>
#include <algorithm>
#include <cmath>
#include <ranges>

FanCurveEditorWidget::FanCurveEditorWidget(QWidget *parent)
    : QWidget(parent)
{
    m_points.clear();
    // initialize 17 evenly spaced points from 20..100 with linear duty 0..100
    const int count = 17;
    for (int i = 0; i < count; ++i) {
        double t = 20.0 + (80.0 * i) / (count - 1); // 20..100
        double d = (100.0 * i) / (count - 1); // 0..100
        m_points.append({t, d});
    }
    setMouseTracking(true);
    setSizePolicy(QSizePolicy::Expanding, QSizePolicy::Expanding);
}

void FanCurveEditorWidget::setPoints(const QVector<Point>& pts) {
    resetInteraction();
    m_points = pts;
    sortPoints();
    // Enforce monotonicity on all points
    for (int i = 0; i < m_points.size(); ++i) {
        enforceMonotonicity(i);
    }
    m_selectedIndices.clear();
    update();
    emit pointsChanged(m_points);
}

void FanCurveEditorWidget::resetInteraction() {
    m_draggedIndex = -1;
    m_dragStartDuties.clear();
    m_rubberBandActive = false;
    m_rubberBandRect = QRect();
    m_selectionBeforeBand.clear();
}

void FanCurveEditorWidget::setEditable(bool editable) {
    resetInteraction();
    m_editable = editable;
    update();
}

double FanCurveEditorWidget::minimumTemperature() const {
    return m_points.isEmpty() ? 20.0 : std::min(20.0, std::floor(m_points.first().temp / 5.0) * 5.0);
}

double FanCurveEditorWidget::dutyAtTemperature(double temp) const
{
    if (m_points.isEmpty())
        return 0.0;
    if (temp <= m_points.first().temp)
        return m_points.first().duty;
    if (temp >= m_points.last().temp)
        return m_points.last().duty;
    for (int i = 1; i < m_points.size(); ++i) {
        const Point &a = m_points[i - 1];
        const Point &b = m_points[i];
        if (temp <= b.temp) {
            const double span = b.temp - a.temp;
            if (span <= 1e-6)
                return b.duty;
            const double frac = (temp - a.temp) / span;
            return std::clamp(a.duty + frac * (b.duty - a.duty), 0.0, 100.0);
        }
    }
    return m_points.last().duty;
}

void FanCurveEditorWidget::setCrosshair( double temp, double duty )
{
    m_crosshairVisible = true;
    m_crosshairTemp = temp;
    m_crosshairDuty = duty;
    if (m_crosshairTrail.isEmpty() ||
        std::abs(m_crosshairTrail.last().temp - temp) >= 0.25 ||
        std::abs(m_crosshairTrail.last().duty - duty) >= 0.25) {
        m_crosshairTrail.append(Point{ temp, duty });
        while (m_crosshairTrail.size() > kMaxTrailPoints)
            m_crosshairTrail.removeFirst();
    }
    update();
}

void FanCurveEditorWidget::clearCrosshair()
{
    m_crosshairVisible = false;
    m_crosshairTrail.clear();
    update();
}

void FanCurveEditorWidget::sortPoints() {
    std::ranges::sort(m_points, {}, &Point::temp);
}

QPointF FanCurveEditorWidget::toWidget(const Point& pt) const {
    // use same margins as paintEvent for consistent mapping
    const int left = 80, right = 20, top = 28, bottom = 68;
    double plotW = width() - left - right;
    double plotH = height() - top - bottom;
    const double minimum = minimumTemperature();
    double x = left + (pt.temp - minimum) / (100.0 - minimum) * plotW;
    double y = top + (1.0 - pt.duty / 100.0) * plotH;
    return QPointF(x, y);
}

FanCurveEditorWidget::Point FanCurveEditorWidget::fromWidget(const QPointF& pos) const {
    const int left = 80, right = 20, top = 28, bottom = 68;
    double plotW = width() - left - right;
    double plotH = height() - top - bottom;
    const double minimum = minimumTemperature();
    double temp = (pos.x() - left) / plotW * (100.0 - minimum) + minimum;
    double duty = (1.0 - (pos.y() - top) / plotH) * 100.0;
    return { std::clamp(temp, minimum, 100.0), std::clamp(duty, 0.0, 100.0) };
}

QRectF FanCurveEditorWidget::pointRect(const Point& pt) const {
    QPointF c = toWidget(pt);
    const double radius = std::clamp((width() - 100.0) / std::max(1LL, static_cast<long long>(m_points.size()) - 1) * 0.35, 3.0, 7.0);
    return QRectF(c.x() - radius, c.y() - radius, radius * 2, radius * 2);
}

void FanCurveEditorWidget::enforceMonotonicity(int modifiedIndex) {
    if (modifiedIndex < 0 || modifiedIndex >= m_points.size()) return;

    double currentDuty = m_points[modifiedIndex].duty;

    // Check all higher temperature points (higher indices)
    for (int i = modifiedIndex + 1; i < m_points.size(); ++i) {
        if (m_points[i].duty < currentDuty) {
            m_points[i].duty = currentDuty;
        }
    }
    // Check all lower temperature points (lower indices) - ensure they are <= current duty
    for (int i = modifiedIndex - 1; i >= 0; --i) {
        if (m_points[i].duty > currentDuty) {
            m_points[i].duty = currentDuty;
        }
    }
}

void FanCurveEditorWidget::resizeEvent(QResizeEvent *event) {
    m_gridCacheDirty = true;
    QWidget::resizeEvent(event);
}

void FanCurveEditorWidget::changeEvent(QEvent *event) {
    if (event) {
        switch (event->type()) {
        case QEvent::PaletteChange:
        case QEvent::StyleChange:
        case QEvent::FontChange:
#if QT_VERSION >= QT_VERSION_CHECK(6, 6, 0)
        case QEvent::DevicePixelRatioChange:
            m_gridCacheDirty = true;
            break;
#endif
        default:
            break;
        }
    }
    QWidget::changeEvent(event);
}

void FanCurveEditorWidget::ensureGridCache(const QRectF &plotRect, double minimum,
                                           const QColor &bgColor, const QColor &gridColor,
                                           const QColor &labelColor) {
    const qreal dpr = devicePixelRatioF();
    const QSize logicalSize = size();
    if (logicalSize.isEmpty())
        return;

    const bool unchanged = !m_gridCacheDirty
        && !m_gridCache.isNull()
        && m_gridCacheSize == logicalSize
        && qFuzzyCompare(m_gridCacheDpr, dpr)
        && qFuzzyCompare(m_gridCacheMinTemp, minimum)
        && m_gridCacheTitle == m_title
        && m_gridCacheBg == bgColor.rgba()
        && m_gridCacheGrid == gridColor.rgba()
        && m_gridCacheLabel == labelColor.rgba();
    if (unchanged)
        return;

    const int left = 80, right = 20, top = 28;
    const QSize pixelSize(qMax(1, qRound(logicalSize.width() * dpr)),
                          qMax(1, qRound(logicalSize.height() * dpr)));
    m_gridCache = QPixmap(pixelSize);
    m_gridCache.setDevicePixelRatio(dpr);
    m_gridCache.fill(bgColor);

    QPainter gp(&m_gridCache);
    gp.setRenderHint(QPainter::Antialiasing);

    // Draw title at the top of the widget if set
    if (!m_title.isEmpty()) {
        QFont titleFont = font();
        titleFont.setPointSize(11);
        titleFont.setWeight(QFont::Bold);
        gp.setFont(titleFont);
        gp.setPen(labelColor);
        QRectF titleRect(left, 2, logicalSize.width() - left - right, top - 4);
        gp.drawText(titleRect, Qt::AlignLeft | Qt::AlignVCenter, m_title);
    }

    // Draw grid and ticks/labels
    QFont tickFont = font();
    tickFont.setPointSize(9);
    tickFont.setWeight(QFont::Normal);
    gp.setFont(tickFont);

    // Y grid/ticks/labels (0-100% every 20%)
    for (int i = 0; i <= 5; ++i) {
        double frac = i / 5.0;
        qreal y = plotRect.top() + (1.0 - frac) * plotRect.height();
        qreal yy = qRound(y) + 0.5;
        gp.setPen(gridColor);
        gp.drawLine(QPointF(qRound(plotRect.left()) + 0.5, yy), QPointF(qRound(plotRect.right()) + 0.5, yy));
        int duty = i * 20;
        QString label = QString::number(duty) + "%";
        gp.setPen(labelColor);
        QRectF labelRect(0, yy - 12, left - 16, 24);
        gp.drawText(labelRect, Qt::AlignRight | Qt::AlignVCenter, label);
    }

    // X grid/ticks/labels (minimum-100 degC)
    const int tickStep = (logicalSize.width() - left - right) < 850 ? 10 : 5;
    for (int temp = static_cast<int>(minimum); temp <= 100; temp += tickStep) {
        double frac = (temp - minimum) / (100.0 - minimum);
        qreal x = plotRect.left() + frac * plotRect.width();
        qreal xx = qRound(x) + 0.5;
        gp.setPen(gridColor);
        gp.drawLine(QPointF(xx, qRound(plotRect.top()) + 0.5), QPointF(xx, qRound(plotRect.bottom()) + 0.5));
        QString label = QString::number(temp) + QChar(0x00B0) + "C";
        gp.setPen(labelColor);
        QRectF labelRect(xx - 20, plotRect.bottom() + 12, 40, 20);
        gp.drawText(labelRect, Qt::AlignHCenter | Qt::AlignTop, label);
    }

    // Axis labels
    QFont axisFont = font();
    axisFont.setPointSize(11);
    axisFont.setWeight(QFont::Normal);
    gp.setPen(labelColor);
    QFont yFont = axisFont;
    yFont.setPointSize(10);
    gp.save();
    int yLabelX = 14;
    gp.translate(yLabelX, plotRect.center().y());
    gp.rotate(-90);
    gp.setFont(yFont);
    QRectF yLabelRect(-plotRect.height() / 2, -12, plotRect.height(), 24);
    gp.drawText(yLabelRect, Qt::AlignCenter, "% Duty");
    gp.restore();

    gp.setFont(axisFont);
    QRectF xLabelRect(plotRect.left(), plotRect.bottom() + 28, plotRect.width(), 20);
    gp.drawText(xLabelRect, Qt::AlignHCenter | Qt::AlignTop, "Temperature (°C)");

    // Draw border around plot area (half-pixel aligned)
    gp.setPen(QPen(labelColor, 1));
    QRectF borderRect(qRound(plotRect.left()) + 0.5, qRound(plotRect.top()) + 0.5,
                      qRound(plotRect.width()) - 1.0, qRound(plotRect.height()) - 1.0);
    gp.drawRect(borderRect);

    m_gridCacheSize = logicalSize;
    m_gridCacheDpr = dpr;
    m_gridCacheMinTemp = minimum;
    m_gridCacheTitle = m_title;
    m_gridCacheBg = bgColor.rgba();
    m_gridCacheGrid = gridColor.rgba();
    m_gridCacheLabel = labelColor.rgba();
    m_gridCacheDirty = false;
}

void FanCurveEditorWidget::paintEvent(QPaintEvent*) {
    QPainter p(this);
    p.setRenderHint(QPainter::Antialiasing);

    const QPalette &pal = palette();
    const QColor bgColor       = pal.color(QPalette::Base);
    const QColor gridColor     = pal.color(QPalette::Mid);
    const QColor labelColor    = pal.color(QPalette::Text);
    const QColor brightText    = pal.color(QPalette::BrightText);
    const QColor disabledFill  = pal.color(QPalette::Disabled, QPalette::Mid);
    const QColor disabledBorder= pal.color(QPalette::Disabled, QPalette::Light);

    // Data-visualization colors: warm tones that won't collide with
    // typical blue-ish GUI highlight/link palette roles.
    const bool darkTheme = bgColor.lightnessF() < 0.5;
    const QColor curveColor    = darkTheme ? QColor( 0x3f, 0xa9, 0xf5 ) : QColor( 0x19, 0x76, 0xd2 );
    const QColor accentColor   = darkTheme ? QColor( 0xff, 0x57, 0x22 ) : QColor( 0xe6, 0x4a, 0x19 );
    const QColor selectedFill  = darkTheme ? QColor( 0xff, 0xa7, 0x26 ) : QColor( 0xfb, 0x8c, 0x00 );
    const QColor selectedBorder= darkTheme ? QColor( 0xff, 0x6f, 0x00 ) : QColor( 0xe6, 0x51, 0x00 );

    // Margins for axes
    int left = 80, right = 20, top = 28, bottom = 68;
    QRectF plotRect(left, top, width() - left - right, height() - top - bottom);
    const double minimum = minimumTemperature();

    // Blit cached static background/grid/axes/labels layer; only dynamic curve,
    // EWMA trajectory trail, and crosshair are painted per tick/drag.
    ensureGridCache(plotRect, minimum, bgColor, gridColor, labelColor);
    if (!m_gridCache.isNull())
        p.drawPixmap(0, 0, m_gridCache);
    else
        p.fillRect(rect(), bgColor);

    QFont tickFont = font();
    tickFont.setPointSize(9);
    tickFont.setWeight(QFont::Normal);

    // Draw curve
    p.setFont(tickFont);
    p.setPen(QPen(curveColor, 3));
    for (int i = 1; i < m_points.size(); ++i) {
        p.drawLine(toWidget(m_points[i-1]), toWidget(m_points[i]));
    }
    // Draw points
    for (int i = 0; i < m_points.size(); ++i) {
        QRectF r = pointRect(m_points[i]);
        if (m_editable) {
            bool selected = m_selectedIndices.contains(i);
            if (selected) {
                p.setBrush(selectedFill);
                p.setPen(QPen(selectedBorder, 2));
            } else {
                p.setBrush(brightText);
                p.setPen(QPen(curveColor, 2));
            }
        } else {
            p.setBrush(disabledFill);
            p.setPen(QPen(disabledBorder, 2));
        }
        p.drawEllipse(r);
    }

    // Draw live crosshair overlay
    if (m_crosshairVisible) {
        Point crossPt { m_crosshairTemp, m_crosshairDuty };
        QPointF cp = toWidget(crossPt);

        // Clamp to plot area
        cp.setX(std::clamp(cp.x(), (double)plotRect.left(), (double)plotRect.right()));
        cp.setY(std::clamp(cp.y(), (double)plotRect.top(), (double)plotRect.bottom()));

        // Draw fading recent EWMA trajectory trail
        if (m_crosshairTrail.size() >= 2) {
            const int n = static_cast<int>(m_crosshairTrail.size());
            for (int i = 1; i < n; ++i) {
                QPointF p0 = toWidget(m_crosshairTrail[i - 1]);
                QPointF p1 = toWidget(m_crosshairTrail[i]);
                p0.setX(std::clamp(p0.x(), static_cast<double>(plotRect.left()), static_cast<double>(plotRect.right())));
                p0.setY(std::clamp(p0.y(), static_cast<double>(plotRect.top()), static_cast<double>(plotRect.bottom())));
                p1.setX(std::clamp(p1.x(), static_cast<double>(plotRect.left()), static_cast<double>(plotRect.right())));
                p1.setY(std::clamp(p1.y(), static_cast<double>(plotRect.top()), static_cast<double>(plotRect.bottom())));
                QColor segColor = accentColor;
                segColor.setAlpha(static_cast<int>(35 + (145 * i) / std::max(1, n - 1)));
                p.setPen(QPen(segColor, 2.0, Qt::SolidLine, Qt::RoundCap));
                p.drawLine(p0, p1);
            }
        }

        // Dashed crosshair lines
        QPen crossPen(accentColor, 1.5, Qt::DashLine);
        p.setPen(crossPen);
        // Vertical line (temperature)
        p.drawLine(QPointF(cp.x(), plotRect.top()), QPointF(cp.x(), plotRect.bottom()));
        // Horizontal line (duty)
        p.drawLine(QPointF(plotRect.left(), cp.y()), QPointF(plotRect.right(), cp.y()));

        // Target prediction marker on the configured curve at current temperature
        const double targetDuty = dutyAtTemperature(m_crosshairTemp);
        QPointF tp = toWidget(Point{ m_crosshairTemp, targetDuty });
        tp.setX(std::clamp(tp.x(), static_cast<double>(plotRect.left()), static_cast<double>(plotRect.right())));
        tp.setY(std::clamp(tp.y(), static_cast<double>(plotRect.top()), static_cast<double>(plotRect.bottom())));
        const double deltaDuty = targetDuty - m_crosshairDuty;
        if (std::abs(deltaDuty) >= 1.0) {
            QColor bridgeColor = curveColor;
            bridgeColor.setAlpha(180);
            p.setPen(QPen(bridgeColor, 2.0, Qt::SolidLine));
            p.drawLine(cp, tp);
            p.setBrush(bgColor);
            p.setPen(QPen(curveColor, 2.0));
            p.drawEllipse(tp, 5.5, 5.5);
        }

        // Draw crosshair dot
        p.setBrush(accentColor);
        p.setPen(QPen(brightText, 1.5));
        p.drawEllipse(cp, 5.0, 5.0);

        // Compact live target readout in top-right header bar (never collides with left title)
        QFont readoutFont = font();
        readoutFont.setPointSize(8);
        readoutFont.setWeight(QFont::DemiBold);
        p.setFont(readoutFont);
        const QString readout = std::abs(deltaDuty) >= 1.0
            ? QString("%1%2C · %3% → Target %4%")
                  .arg(qRound(m_crosshairTemp))
                  .arg(QChar(0x00B0))
                  .arg(qRound(m_crosshairDuty))
                  .arg(qRound(targetDuty))
            : QString("%1%2C · %3% (on curve)")
                  .arg(qRound(m_crosshairTemp))
                  .arg(QChar(0x00B0))
                  .arg(qRound(m_crosshairDuty));
        p.setPen(accentColor);
        QRectF readoutRect(left, 2, width() - left - right, top - 4);
        p.drawText(readoutRect, Qt::AlignRight | Qt::AlignVCenter, readout);

        // Draw labels
        QFont labelFont = font();
        labelFont.setPointSize(8);
        labelFont.setWeight(QFont::Bold);
        p.setFont(labelFont);

        // Temperature label (below X axis at crosshair X)
        QString tempLabel = QString::number(m_crosshairTemp, 'f', 0) + QChar(0x00B0) + "C";
        p.setPen(accentColor);
        QRectF tempLabelRect(cp.x() - 20, plotRect.bottom() + 1, 40, 14);
        p.fillRect(tempLabelRect, bgColor);
        p.drawText(tempLabelRect, Qt::AlignHCenter | Qt::AlignTop, tempLabel);

        // Duty label (left of Y axis at crosshair Y)
        QString dutyLabel = QString::number(m_crosshairDuty, 'f', 0) + "%";
        QRectF dutyLabelRect(plotRect.left() - 40, cp.y() - 7, 38, 14);
        // Clear the entire tick-label area before showing the live value.
        // At 100% a narrow mask otherwise leaves the first digit of the tick.
        p.fillRect(QRectF(0, cp.y() - 12, left - 2, 24), bgColor);
        p.drawText(dutyLabelRect, Qt::AlignRight | Qt::AlignVCenter, dutyLabel);
    }

    // Draw rubber band selection rectangle
    if (m_rubberBandActive && m_rubberBandRect.isValid()) {
        QColor bandFill = selectedFill;
        bandFill.setAlpha(40);
        QColor bandBorder = selectedFill;
        bandBorder.setAlpha(160);
        p.setBrush(bandFill);
        p.setPen(QPen(bandBorder, 1, Qt::DashLine));
        p.drawRect(m_rubberBandRect);
    }
}

void FanCurveEditorWidget::mousePressEvent(QMouseEvent* e) {
    if (!m_editable || e->button() != Qt::LeftButton) return;
    resetInteraction();
    m_ctrlHeld = (e->modifiers() & Qt::ControlModifier);

    // Check if clicked on a point
    int hitIndex = -1;
    double nearest = 100.0;
    for (int i = 0; i < m_points.size(); ++i) {
        const auto offset = toWidget(m_points[i]) - e->position();
        const double distance = QPointF::dotProduct(offset, offset);
        if (distance < nearest) {
            hitIndex = i;
            nearest = distance;
        }
    }

    if (hitIndex >= 0) {
        if (m_ctrlHeld) {
            // Ctrl+click: toggle selection
            if (m_selectedIndices.contains(hitIndex))
                m_selectedIndices.remove(hitIndex);
            else
                m_selectedIndices.insert(hitIndex);
            update();
            return;
        } else {
            // Plain click on a point: if not already selected, replace selection
            if (!m_selectedIndices.contains(hitIndex)) {
                m_selectedIndices.clear();
                m_selectedIndices.insert(hitIndex);
            }
        }
        // Start dragging the entire selection
        m_draggedIndex = hitIndex;
        m_dragStartY = e->pos().y();
        m_dragStartDuties.resize(m_points.size());
        for (int i = 0; i < m_points.size(); ++i)
            m_dragStartDuties[i] = m_points[i].duty;
        update();
        return;
    }

    // Clicked on empty space - start rubber band
    if (!m_ctrlHeld)
        m_selectedIndices.clear();
    m_selectionBeforeBand = m_selectedIndices;
    m_rubberBandActive = true;
    m_rubberBandOrigin = e->pos();
    m_rubberBandRect = QRect();
    update();
}

void FanCurveEditorWidget::mouseMoveEvent(QMouseEvent* e) {
    if (!m_editable || !(e->buttons() & Qt::LeftButton)) return;

    // Rubber band selection
    if (m_rubberBandActive) {
        m_rubberBandRect = QRect(m_rubberBandOrigin, e->pos()).normalized();
        // Preview: select all points inside the rubber band
        m_selectedIndices = m_selectionBeforeBand;
        for (int i = 0; i < m_points.size(); ++i) {
            QPointF wp = toWidget(m_points[i]);
            if (m_rubberBandRect.contains(wp.toPoint()))
                m_selectedIndices.insert(i);
        }
        update();
        return;
    }

    // Dragging selected points
    if (m_draggedIndex < 0) return;
    dragTo(e->position());
}

void FanCurveEditorWidget::dragTo(const QPointF& position) {
    if (m_draggedIndex < 0 || m_dragStartDuties.size() != m_points.size()) return;

    const int top = 28, bottom = 68;
    double plotH = height() - top - bottom;
    double deltaY = position.y() - m_dragStartY;
    double deltaDuty = -(deltaY / plotH) * 100.0;

    // Rebuild every preview from the press snapshot. Otherwise monotonicity
    // edits to neighbouring points accumulate and cannot be undone by dragging back.
    for (int i = 0; i < m_points.size(); ++i)
        m_points[i].duty = m_dragStartDuties[i];

    for (int idx : m_selectedIndices) {
        if (idx < 0 || idx >= m_points.size()) continue;
        double newDuty = m_dragStartDuties[idx] + deltaDuty;
        newDuty = std::round(std::clamp(newDuty, 0.0, 100.0));
        m_points[idx].duty = newDuty;
    }

    // Enforce monotonicity from each selected point
    // Process from lowest to highest index
    QList<int> sorted = m_selectedIndices.values();
    std::ranges::sort(sorted);
    for (int idx : sorted) {
        enforceMonotonicity(idx);
    }

    update();
    emit pointsChanged(m_points);
}

void FanCurveEditorWidget::mouseReleaseEvent(QMouseEvent* e) {
    if (e->button() != Qt::LeftButton) return;
    if (m_draggedIndex >= 0) dragTo(e->position());
    if (m_rubberBandActive) {
        m_rubberBandActive = false;
        m_rubberBandRect = QRect();
        update();
    }
    resetInteraction();
}

void FanCurveEditorWidget::contextMenuEvent(QContextMenuEvent*) {
    // No context menu - points are fixed
}

void FanCurveEditorWidget::addPoint(const Point& pt) {
    // Snap temperature to nearest 5 degC grid position
    double snappedTemp = std::round((pt.temp - 20.0) / 5.0) * 5.0 + 20.0;
    snappedTemp = std::clamp(snappedTemp, 20.0, 100.0);

    // Check if a point already exists at this temperature
    for (const auto& existingPt : m_points) {
        if (std::abs(existingPt.temp - snappedTemp) < 1.0) {
            return; // Don't add duplicate
        }
    }

    m_points.push_back({snappedTemp, pt.duty});
    sortPoints();

    // Find the index of the newly added point and enforce monotonicity
    int newIndex = -1;
    for (int i = 0; i < m_points.size(); ++i) {
        if (std::abs(m_points[i].temp - snappedTemp) < 1.0) {
            newIndex = i;
            break;
        }
    }
    if (newIndex >= 0) {
        enforceMonotonicity(newIndex);
    }

    update();
    emit pointsChanged(m_points);
}

void FanCurveEditorWidget::removePoint(int idx) {
    if (idx > 0 && idx < m_points.size()-1 && m_points.size() > 9) {
        m_points.remove(idx);
        update();
        emit pointsChanged(m_points);
    }
}
