#include "PreviewMode.hpp"
#include "FluentTheme.hpp"
/*
 * This program is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * This program is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with this program.  If not, see <https://www.gnu.org/licenses/>.
 */

#include "MonitorTab.hpp"
#include "AsyncRead.hpp"
#include "../libucc-dbus/UccdClient.hpp"
#include <QDateTime>
#include <QFile>
#include <QLabel>
#include <QScrollArea>
#include <QGridLayout>
#include <QStackedWidget>
#include <QToolTip>
#include <QDir>
#include <QSettings>
#include <QRegularExpression>
#include <QMainWindow>
#include <QStatusBar>
#include <QApplication>
#include <QGraphicsLayout>
#include <QPainter>
#include <QJsonDocument>
#include <QJsonArray>
#include <QJsonObject>
#include <cmath>
#include <cstring>
#include <algorithm>
#include <functional>

namespace ucc
{

// Clickable graphics rect - calls a callback on mouse press

class ClickableRectItem : public QGraphicsRectItem
{
public:
  explicit ClickableRectItem( QGraphicsItem *parent = nullptr )
    : QGraphicsRectItem( parent )
  {
    setAcceptedMouseButtons( Qt::LeftButton );
    setCursor( Qt::PointingHandCursor );
  }

  void setClickCallback( std::function< void() > cb ) { m_onClick = std::move( cb ); }

protected:
  void mousePressEvent( QGraphicsSceneMouseEvent *event ) override
  {
    event->accept();
    if ( m_onClick )
    {
      auto cb = m_onClick;
      QTimer::singleShot( 0, [cb = std::move( cb )]() { cb(); } );
    }
  }

private:
  std::function< void() > m_onClick;
};

// Metric definitions - higher-contrast palette with more greens to improve
// readability between neighbouring lines.
struct MetricDef
{
  const char *key;       // JSON key from MetricsHistoryStore
  const char *label;     // Human-readable label
  QColor      color;     // Line colour
  MetricGroup group;
};

static constexpr int METRIC_COUNT = 10;

// Order matches MetricId enum in MetricsHistoryStore.hpp
static const MetricDef kMetrics[ METRIC_COUNT ] =
{
  { "cpuTemp",             "CPU Temp",            QColor("#0067C0"),  MetricGroup::Temp  },
  { "cpuFanDuty",          "CPU Fan Duty",        QColor("#0067C0"),   MetricGroup::Duty  },
  { "cpuPower",            "CPU Power",           QColor("#0067C0"), MetricGroup::Power },
  { "cpuFrequency",        "CPU Frequency",       QColor("#0067C0"),    MetricGroup::Freq  },
  { "gpuTemp",             "dGPU Temp",           QColor("#008C95"),   MetricGroup::Temp  },
  { "gpuFanDuty",          "dGPU Fan Duty",       QColor("#008C95"),   MetricGroup::Duty  },
  { "gpuPower",            "dGPU Power",          QColor("#008C95"),  MetricGroup::Power },
  { "gpuFrequency",        "dGPU Frequency",      QColor("#008C95"),  MetricGroup::Freq  },
  { "gpuVramFrequency",    "dGPU VRAM Freq",      QColor("#7956B8"),  MetricGroup::Freq  },
  { "gpuCoreVoltage",      "dGPU Core Voltage",   QColor("#008C95"),   MetricGroup::Volt  },
};

// Helpers

/**
 * @brief Undo the normalisation applied to a metric value to restore its real value.
 * Inverse of metricToNormalisedScale().
 */
double MonitorTab::metricFromNormalisedScale( double normalisedValue, MetricGroup g )
{
  switch ( g )
  {
    case MetricGroup::Temp:  return normalisedValue / (100.0 / 105.0);  // 0-100 -> 0-105 degC
    case MetricGroup::Duty:  return normalisedValue;                     // already %
    case MetricGroup::Power: return normalisedValue / (100.0 / m_maxPowerW);  // 0-100 -> 0-m_maxPowerW W
    case MetricGroup::Freq:  return normalisedValue / (100.0 / 6000.0);  // 0-100 -> 0-6000 MHz
    case MetricGroup::Volt:  return normalisedValue / (100.0 / 1500.0);  // 0-100 -> 0-1500 mV
  }
  return normalisedValue;
}

/**
 * @brief Scale factor for normalising a metric group value to [0, 100].
 * Used by the unified chart's shadow series (accesses this->m_maxPowerW for Power group).
 */
double MonitorTab::metricToNormalisedScale( MetricGroup g )
{
  switch ( g )
  {
    case MetricGroup::Temp:  return 100.0 / 105.0;  // 0-105 degC -> 0-100
    case MetricGroup::Duty:  return 1.0;             // already %
    case MetricGroup::Power: return 100.0 / m_maxPowerW;  // 0-m_maxPowerW W -> 0-100
    case MetricGroup::Freq:  return 100.0 / 6000.0;  // 0-6000 MHz-> 0-100
    case MetricGroup::Volt:  return 100.0 / 1500.0;  // 0-1500 mV -> 0-100
  }
  return 1.0;
}

static const char *metricGroupUnit( MetricGroup g )
{
  switch ( g )
  {
    case MetricGroup::Temp:  return "°C";
    case MetricGroup::Duty:  return "%";
    case MetricGroup::Power: return "W";
    case MetricGroup::Freq:  return "MHz";
    case MetricGroup::Volt:  return "mV";
  }
  return "";
}

void MonitorTab::initializeMaxPowerFromHardware()
{
  // Get GPU TGP (cTGP) maximum
  int maxGpuTgp = 0;
  int maxBoostTdp = 0;

  if ( auto gpuMax = m_client->getNVIDIAPowerCTRLMaxPowerLimit() )
    maxGpuTgp = *gpuMax;

  // Get boost TDP maximum (index 1 from ODM power limits)
  if ( auto tdpLimits = m_client->getODMPowerLimits() )
  {
    // Index 1 is typically "Boost TDP"
    if ( tdpLimits->size() > 1 )
      maxBoostTdp = (*tdpLimits)[1];
  }

  if ( m_maxPowerW = std::max( maxGpuTgp, maxBoostTdp ); m_maxPowerW <= 0 )
    m_maxPowerW = 200;  // Fallback to default
}

static QColor metricColor(const MetricDef &metric) {
  if(!FluentTheme::isDark()) return metric.color;
  if(QString::fromUtf8(metric.key).startsWith("cpu")) return QColor("#75BAFF");
  if(QString::fromUtf8(metric.key)=="gpuVramFrequency") return QColor("#C4A4FF");
  return QColor("#69D4D8");
}

static QChart *createChart()
{
  auto *chart = new QChart();
  // Don't set title - will save vertical space for graphs
  chart->setAnimationOptions( QChart::NoAnimation );
  chart->legend()->setVisible(false);
  chart->legend()->setAlignment(Qt::AlignTop);
  QFont legendFont=QApplication::font();legendFont.setPointSize(8);chart->legend()->setFont(legendFont);
  chart->setMargins(QMargins(0,0,0,0));
  chart->layout()->setContentsMargins(0,0,0,0);

  // Light chart background
  chart->setBackgroundBrush( QBrush( FluentTheme::colors().surface ) );
  chart->setPlotAreaBackgroundBrush( QBrush( FluentTheme::colors().surface ) );
  chart->setPlotAreaBackgroundVisible( true );
  chart->setTitleBrush( QBrush( FluentTheme::colors().text ) );

  return chart;
}

static void styleAxis( QAbstractAxis *axis )
{
  QFont labels=QApplication::font();labels.setPointSize(8);axis->setLabelsFont(labels);
  axis->setLabelsBrush( QBrush( FluentTheme::colors().secondary ) );
  axis->setTitleBrush( QBrush( FluentTheme::colors().text ) );
  QPen linePen( FluentTheme::colors().border );
  axis->setLinePen( linePen );
  axis->setGridLinePen( QPen( FluentTheme::colors().border ) );
  axis->setShadesBrush( QBrush( Qt::transparent ) );
}

static QDateTimeAxis *createXAxis()
{
  auto *axis = new QDateTimeAxis();
  axis->setFormat("HH:mm");
  axis->setTickCount( 6 );
  styleAxis( axis );
  return axis;
}

static QValueAxis *createYAxis( const QString &title, double min, double max )
{
  auto *axis = new QValueAxis();
  Q_UNUSED(title);
  axis->setTitleText(QString());
  axis->setTickCount(3);
  axis->setRange( min, max );
  axis->setLabelFormat( "%.0f" );
  styleAxis( axis );
  return axis;
}

static QChartView *createChartView( QChart *chart )
{
  auto *view = new QChartView( chart );
  view->setRenderHint( QPainter::Antialiasing );
  view->setMinimumHeight(90);
  view->setFrameShape(QFrame::NoFrame);
  return view;
}

// Construction

MonitorTab::MonitorTab( UccdClient *client, QWidget *parent )
  : QWidget( parent )
  , m_client( client )
{
  initializeMaxPowerFromHardware();
  setupUI();
  fetchCpuTopology();
  connect(FluentTheme::events(),&FluentTheme::ThemeEvents::changed,this,&MonitorTab::refreshTheme);
  refreshTheme();

  setFocusPolicy( Qt::StrongFocus );  // Enable keyboard events for spacebar pause

  m_fetchTimer.setInterval( 2000 );
  connect( &m_fetchTimer, &QTimer::timeout, this, &MonitorTab::fetchData );
  // Scroll the view independently of polling. Values remain real samples;
  // the one-poll presentation delay reveals their connecting segments smoothly.
  m_frameTimer.setInterval( 16 );
  m_frameTimer.setTimerType( Qt::PreciseTimer );
  connect( &m_frameTimer, &QTimer::timeout, this, &MonitorTab::renderFrame );
}

void MonitorTab::refreshTheme() {
  const auto c=FluentTheme::colors();
  for(auto *chart:{m_tempChart,m_dutyChart,m_powerChart,m_freqChart,m_voltChart,m_unifiedChart}) {
    if(!chart) continue;
    chart->setBackgroundBrush(c.surface);chart->setPlotAreaBackgroundBrush(c.surface);chart->setTitleBrush(c.text);
    for(auto *axis:chart->axes()) styleAxis(axis);
    for(auto *abstract:chart->series()) if(auto *series=qobject_cast<QLineSeries*>(abstract)) {
      const int index=metricIndexForKey(series->property("_metricKey").toString().toStdString());
      if(index>=0){auto pen=series->pen();pen.setColor(metricColor(kMetrics[index]));series->setPen(pen);}
    }
  }
  if ( m_coreLoadGrid )
    m_coreLoadGrid->update();
  if (m_coreLoadSwatch) {
    m_coreLoadSwatch->setStyleSheet("border: none; border-radius: 1px; background: " + metricColor(kMetrics[0]).name() + ";");
  }
  for(auto *swatch:findChildren<QFrame*>()) if(swatch->property("metricIndex").isValid())
    swatch->setStyleSheet("border: none; border-radius: 1px; background: "+metricColor(kMetrics[swatch->property("metricIndex").toInt()]).name()+";");
  for(auto &[chart,callout]:m_callouts) {
    Q_UNUSED(chart);callout.bg->setBrush(c.inset);callout.bg->setPen(QPen(c.border));callout.text->setBrush(c.text);
  }
  for(auto &mark:m_stickyMarks) {
    for(size_t i=0;i<mark.groupGfxList.size();++i) {
      auto &gfx=mark.groupGfxList[i];if(!gfx.bg) continue;
      const int index=metricIndexForKey(mark.entries[i].metricKey);
      gfx.bg->setBrush(c.inset);gfx.bg->setPen(QPen(index>=0 ? metricColor(kMetrics[index]) : c.border,2));gfx.text->setBrush(c.text);
    }
    if(mark.uniBg){mark.uniBg->setBrush(c.inset);mark.uniBg->setPen(QPen(c.border));}
    for(size_t i=0;i<mark.uniTexts.size();++i) {
      const int index=i>0 ? metricIndexForKey(mark.entries[i-1].metricKey) : -1;
      mark.uniTexts[i]->setBrush(index>=0 ? metricColor(kMetrics[index]) : c.text);
    }
  }
  hideCrosshair();updateMonitorStatus();update();
}

void MonitorTab::syncFrameTimer()
{
  if (m_monitoringActive && !m_paused && isVisible())
    m_frameTimer.start();
  else
    m_frameTimer.stop();
}

void MonitorTab::showEvent( QShowEvent *event )
{
  QWidget::showEvent(event);
  syncFrameTimer();
}

void MonitorTab::hideEvent( QHideEvent *event )
{
  QWidget::hideEvent(event);
  m_frameTimer.stop();
}

void MonitorTab::renderFrame()
{
  if (!m_monitoringActive || m_paused || !isVisible()) return;
  updateAxes();
  updateStickyMarkPositions();
  if (m_cursorInPlot)
    updateCrosshair(m_lastCrosshairPos,m_annotationsVisible);
}

void MonitorTab::setMonitoringActive( bool active )
{
  if (m_monitoringActive == active) return;
  m_monitoringActive = active;
  syncFrameTimer();
  updateMonitorStatus();
  ++m_fetchGeneration;
  if ( active )
  {
    // Clear all in-memory buffers and series to avoid overlapping time ranges
    // (which cause crossed lines when the same timestamps appear twice).
    for ( auto &[key, info] : m_seriesMap )
    {
      info.buffer.clear();
      info.lastTimestamp = 0;
      info.series->clear();

      QVariant uv = info.series->property( "_uniSeries" );
      if ( uv.isValid() )
      {
        auto *uni = qobject_cast< QLineSeries * >( uv.value< QObject * >() );
        if ( uni )
          uni->clear();
      }
    }
    m_prevCoreSamples.clear();
    if ( m_coreLoadGrid )
      m_coreLoadGrid->clearLoads();

    // Only fetch data that fits in the current visible window - not the full
    // daemon history horizon (which can be 30 minutes). This bounds the
    // initial render cost to m_windowSeconds worth of points.
    const qint64 now = QDateTime::currentMSecsSinceEpoch();
    m_lastTimestamp = now - m_fetchTimer.interval() - static_cast< qint64 >( m_windowSeconds ) * 1000;
    sampleAndUpdateCoreLoad();
    fetchData();
    m_fetchTimer.start();
    m_unifiedChartView->setFocus();  // Immediate key events (crosshair Ctrl)
  }
  else
  {
    m_fetchTimer.stop();
  }
}

// UI Setup

void MonitorTab::updateMonitorStatus()
{
  if (!m_pauseLabel) return;
  QString color = FluentTheme::colors().secondary.name();
  QString text = "Idle";
  if (!m_client->isConnected()) text = "Waiting for service";
  else if (m_paused) { text = "Ⅱ Paused"; color = FluentTheme::colors().warning.name(); }
  else if (m_monitoringActive) { text = "● Live · 2 s delay"; color = FluentTheme::colors().success.name(); }
  m_pauseLabel->setText(text);
  m_pauseLabel->setStyleSheet("QLabel { color: " + color + "; font-weight: bold; padding: 8px; }");
}

void MonitorTab::setupUI()
{
  auto *mainLayout = new QHBoxLayout( this );
  mainLayout->setContentsMargins(0,0,0,0);mainLayout->setSpacing(16);

  setupControls();

  // The control panel is secondary to the telemetry plots.
  auto *legendBox = FluentTheme::createCard();
  legendBox->setFixedWidth(205);
  auto *legendLayout = new QGridLayout( legendBox );
  legendLayout->setContentsMargins(18,20,18,20);
  legendLayout->setVerticalSpacing(12);
  legendLayout->setHorizontalSpacing(12);
  auto *panelTitle=new QLabel("Visible metrics");panelTitle->setObjectName("cardTitle");legendLayout->addWidget(panelTitle,0,0,1,2);
  int col = 0, row = 1;
  for ( int i = 0; i < METRIC_COUNT; ++i )
  {
    const auto &md = kMetrics[ i ];
    if (i==0 || i==4) {
      auto *group=new QLabel(i==0 ? "CPU" : "GPU");group->setObjectName("subheading");
      legendLayout->setRowMinimumHeight(row,32);legendLayout->addWidget(group,row++,0,1,2);
    }
    QString controlLabel=QString::fromUtf8(md.label);
    controlLabel.remove(QRegularExpression("^(CPU |dGPU )"));
    controlLabel.replace("Temp","Temperature").replace("Fan Duty","Fan duty").replace("VRAM Freq","VRAM frequency").replace("Core Voltage","Core voltage");
    auto *cb = new QCheckBox(controlLabel);cb->setToolTip(md.label);
    cb->setChecked( true );
    auto *swatch=new QFrame;swatch->setFixedSize(20,3);swatch->setProperty("metricIndex",i);
    swatch->setStyleSheet("border: none; border-radius: 1px; background: " + metricColor(md).name() + ";");
    legendLayout->addWidget(swatch,row,1);

    legendLayout->addWidget( cb, row, col );
    ++row;

    // Create series
    auto *series = new QLineSeries();
    series->setName( md.label );
    QPen pen( metricColor(md) );
    pen.setWidth( 1 );
    series->setPen( pen );
    series->setProperty( "_unit", QString::fromUtf8( metricGroupUnit( md.group ) ) );
    series->setProperty( "_metricKey", QString::fromStdString( md.key ) );

    connect( cb, &QCheckBox::toggled, series, &QLineSeries::setVisible );

    m_seriesMap[ md.key ] = { series, cb, md.label, metricColor(md), {} };

    if ( i == 3 )
    {
      m_coreLoadCheckBox = new QCheckBox( "Core load" );
      m_coreLoadCheckBox->setObjectName( "monitorCpuCoreLoad" );
      m_coreLoadCheckBox->setToolTip( "Per-core CPU utilization (%)" );
      m_coreLoadCheckBox->setChecked( true );
      m_coreLoadSwatch = new QFrame;
      m_coreLoadSwatch->setFixedSize( 20, 3 );
      m_coreLoadSwatch->setStyleSheet( "border: none; border-radius: 1px; background: " + metricColor(kMetrics[0]).name() + ";" );
      legendLayout->addWidget( m_coreLoadSwatch, row, 1 );
      legendLayout->addWidget( m_coreLoadCheckBox, row, col );
      ++row;
    }
  }

  m_unifiedCheckBox = new QCheckBox( "Unified Graph" );
  m_unifiedCheckBox->setChecked( false );
  connect( m_unifiedCheckBox, &QCheckBox::toggled, this, &MonitorTab::setUnifiedMode );
  legendLayout->setRowMinimumHeight(row,44);
  legendLayout->addWidget( m_unifiedCheckBox, row, col,1,2 );

  ++row;
  // Pause indicator (hidden by default, shown when spacebar pauses updates)
  m_pauseLabel = new QLabel;
  m_pauseLabel->setObjectName("monitorStatus");
  m_pauseLabel->setWordWrap(true);
  updateMonitorStatus();
  connect(m_client, &UccdClient::connectionStatusChanged, this, [this](bool connected) {
    updateMonitorStatus();
    if (connected && m_coreTypes.isEmpty())
      fetchCpuTopology();
  });
  legendLayout->addWidget( m_pauseLabel, row, col,1,2 );

  legendLayout->setRowStretch(row+1,1);
  mainLayout->addWidget(legendBox);

  // Per-group page (4 charts filling available space)
  auto *chartsWidget = new QWidget();
  auto *chartsLayout = new QVBoxLayout( chartsWidget );
  chartsLayout->setContentsMargins( 0, 0, 0, 0 );
  chartsLayout->setSpacing(12);

  setupTemperatureChart();
  setupDutyChart();
  setupPowerChart();
  setupFrequencyChart();
  setupVoltageChart();
  setupCoreLoadCard();

  auto addChartCard = [chartsLayout](QChartView *view, const QString &title) {
    auto *card = FluentTheme::createCard();
    auto *layout = new QVBoxLayout(card);
    layout->setContentsMargins(12,10,12,8);
    layout->setSpacing(0);
    auto *header = new QHBoxLayout;
    header->setSpacing(12);
    auto *heading = new QLabel(title);
    heading->setObjectName("cardTitle");
    header->addWidget(heading);
    header->addStretch();
    for (auto *abstractSeries : view->chart()->series()) {
      auto *series = qobject_cast<QLineSeries *>(abstractSeries);
      if (!series) continue;
      QString name = series->name().startsWith("CPU") ? "CPU" : "dGPU";
      if (series->name().contains("VRAM")) name = "VRAM";
      auto *label = new QLabel("━ " + name);
      label->setToolTip(series->name());
      label->setStyleSheet("font-size: 12px; color: " + series->color().name() + ";");
      header->addWidget(label);
      QObject::connect(series,&QXYSeries::colorChanged,label,[label](QColor color){label->setStyleSheet("font-size: 12px; color: "+color.name()+";");});
      QObject::connect(series, &QAbstractSeries::visibleChanged, label,
                       [series,label] { label->setVisible(series->isVisible()); });
    }
    layout->addLayout(header);
    layout->addWidget(view,1);
    chartsLayout->addWidget(card,1);
  };
  addChartCard(m_tempChartView, "Temperature · °C");
  addChartCard(m_dutyChartView, "Fan duty · %");
  addChartCard(m_powerChartView, "Power · W");
  addChartCard(m_freqChartView, "Frequency · MHz");
  addChartCard(m_voltChartView, "Core voltage · mV");
  if ( m_coreLoadCard )
    chartsLayout->addWidget( m_coreLoadCard, 1 );

  // Wrap the per-group charts in a scroll area so the window can be
  // resized smaller than the combined minimum height of 4 charts.
  auto *scrollArea = new QScrollArea();
  scrollArea->setWidget( chartsWidget );
  scrollArea->setWidgetResizable( true );
  scrollArea->setFrameShape( QFrame::NoFrame );
  scrollArea->setContentsMargins( 0, 0, 0, 0 );

  m_perGroupPage = scrollArea;  // stacked page 0

  // Unified "all-in-one" chart (page 1)
  setupUnifiedChart();

  // Stacked widget to switch between the two views
  m_chartStack = new QStackedWidget();
  m_chartStack->addWidget( m_perGroupPage );      // index 0
  m_chartStack->addWidget( m_unifiedChartView );   // index 1
  m_chartStack->setCurrentIndex( 0 );

  mainLayout->addWidget( m_chartStack, 1 );

  // Install hover callout on every chart
  installHoverCallout( m_tempChart );
  installHoverCallout( m_dutyChart );
  installHoverCallout( m_powerChart );
  installHoverCallout( m_freqChart );
  installHoverCallout( m_voltChart );
  // Unified chart hover is installed lazily in setUnifiedMode()
  // because its series don't exist yet at this point.

  // Load saved checkbox states
  loadCheckboxStates();

  // Connect all checkboxes to auto-save on toggle and update group visibility
  for ( auto &[key, info] : m_seriesMap )
  {
    connect( info.toggle, &QCheckBox::toggled,
             this, &MonitorTab::saveCheckboxStates );
    connect( info.toggle, &QCheckBox::toggled,
             this, &MonitorTab::updateGroupChartVisibility );
    connect( info.toggle, &QCheckBox::toggled,
             this, &MonitorTab::updateStickyMarkPositions );
  }
  if ( m_coreLoadCheckBox )
  {
    connect( m_coreLoadCheckBox, &QCheckBox::toggled,
             this, &MonitorTab::saveCheckboxStates );
    connect( m_coreLoadCheckBox, &QCheckBox::toggled,
             this, &MonitorTab::updateGroupChartVisibility );
  }

  // Apply initial group visibility after loading saved states
  updateGroupChartVisibility();
}

void MonitorTab::setupControls()
{
  // Time window is controlled by mouse scroll; no combo box needed.
}

void MonitorTab::setTimeWindow( int seconds )
{
  m_windowSeconds = std::clamp( seconds, 60, 1800 );
  ++m_fetchGeneration;

  // Update the main window status bar
  if ( auto *mw = qobject_cast< QMainWindow * >( window() ) )
  {
    const int mins = m_windowSeconds / 60;
    const int secs = m_windowSeconds % 60;
    QString text;
    if ( secs == 0 )
      text = QStringLiteral( "Time window: %1 min" ).arg( mins );
    else
      text = QStringLiteral( "Time window: %1:%2" )
          .arg( mins ).arg( secs, 2, 10, QLatin1Char( '0' ) );
    mw->statusBar()->showMessage( text, 3000 );
  }

  if ( m_paused )
  {
    // When paused we cannot re-fetch, so just shift the visible axis range.
    // Do NOT trim or clear buffers - the data must survive zoom-in so that a
    // subsequent zoom-out can reveal it again.
    updateAxes();
    updateStickyMarkPositions();
  }
  else
  {
    // Clear all in-memory buffers and re-fetch from the new horizon so that
    // widening the window loads fresh history and narrowing immediately trims.
    for ( auto &[key, info] : m_seriesMap )
    {
      info.buffer.clear();
      info.lastTimestamp = 0;
      info.series->clear();
      QVariant uv = info.series->property( "_uniSeries" );
      if ( uv.isValid() )
        if ( auto *uni = qobject_cast< QLineSeries * >( uv.value< QObject * >() ) )
          uni->clear();
    }
    m_prevCoreSamples.clear();
    if ( m_coreLoadGrid )
      m_coreLoadGrid->clearLoads();
    const qint64 now = QDateTime::currentMSecsSinceEpoch();
    m_lastTimestamp = now - m_fetchTimer.interval() - static_cast< qint64 >( m_windowSeconds ) * 1000;
    sampleAndUpdateCoreLoad();
    fetchData();
    updateAxes();
  }

  // Preview never persists local preferences.

  if (readOnlyPreview) return;
  // Persist only the time window value
  QSettings settings( QDir::homePath() + "/.config/uccrc", QSettings::IniFormat );
  settings.beginGroup( "MonitorTab" );
  settings.setValue( "TimeWindowSeconds", m_windowSeconds );
  settings.endGroup();
  settings.sync();
}

void MonitorTab::wheelEvent( QWheelEvent *event )
{
  // Only change the time window when Ctrl is held; otherwise let the
  // event propagate normally so the scroll area can scroll.
  if ( !( event->modifiers() & Qt::ControlModifier ) )
  {
    QWidget::wheelEvent( event );
    return;
  }

  // Ctrl + scroll: each 120-unit notch changes the window by 30 seconds
  const int delta = event->angleDelta().y();
  if ( delta == 0 )
    return;

  // Scroll up = zoom in (shorter window), scroll down = zoom out (longer)
  const int step = ( delta > 0 ) ? -30 : 30;
  setTimeWindow( m_windowSeconds + step );
  event->accept();
}

// Unified chart - normalised 0-100 % Y axis

void MonitorTab::setupUnifiedChart()
{
  m_unifiedChart = createChart();
  m_unifiedXAxis = createXAxis();
  m_unifiedYAxis = createYAxis( "%", 0, 100 );
  m_unifiedChart->addAxis( m_unifiedXAxis, Qt::AlignBottom );
  m_unifiedChart->addAxis( m_unifiedYAxis, Qt::AlignLeft );
  m_unifiedChart->legend()->setVisible( false );

  // Add an invisible anchor series so that the axes render their labels
  // even before the real (lazy) shadow series are created.
  auto *anchor = new QLineSeries();
  anchor->setVisible( false );
  anchor->setPen( QPen( Qt::transparent ) );
  m_unifiedChart->addSeries( anchor );
  anchor->attachAxis( m_unifiedXAxis );
  anchor->attachAxis( m_unifiedYAxis );

  // Shadow series created lazily in createUnifiedSeries() when the
  // unified view is first activated.
  m_unifiedChartView = createChartView( m_unifiedChart );

  // Crosshair line (hidden by default)
  m_crosshairLine = new QGraphicsLineItem( m_unifiedChart );
  m_crosshairLine->setPen( QPen( QColor( 200, 200, 200, 150 ), 1, Qt::DashLine ) );
  m_crosshairLine->setZValue( 80 );
  m_crosshairLine->hide();

  // Enable mouse tracking so we get move events without button press
  m_unifiedChartView->setMouseTracking( true );
  m_unifiedChartView->viewport()->setMouseTracking( true );
  m_unifiedChartView->viewport()->installEventFilter( this );
  m_unifiedChartView->setFocusPolicy( Qt::StrongFocus );
}

void MonitorTab::createUnifiedSeries()
{
  if ( m_unifiedSeriesActive )
    return;  // Already created

  // Create a shadow normalised series for each metric and synchronise it
  // with the original series via property mirroring in applyBinaryData().
  for ( int i = 0; i < METRIC_COUNT; ++i )
  {
    const auto &md = kMetrics[ i ];
    auto *ns = new QLineSeries();
    ns->setName( md.label );
    QPen pen( metricColor(md) );
    pen.setWidth( 1 );
    ns->setPen( pen );

    m_unifiedChart->addSeries( ns );
    ns->attachAxis( m_unifiedXAxis );
    ns->attachAxis( m_unifiedYAxis );

    // Store reverse-scale and unit on the shadow series
    const double fwdScale = metricToNormalisedScale( md.group );
    ns->setProperty( "_realScale", 1.0 / fwdScale );
    ns->setProperty( "_unit", QString::fromUtf8( metricGroupUnit( md.group ) ) );
    ns->setProperty( "_metricKey", QString::fromStdString( md.key ) );

    // Store the shadow series on the original for applyBinaryData/trimSeries
    m_seriesMap[ md.key ].series->setProperty( "_uniSeries",
        QVariant::fromValue( static_cast< QObject * >( ns ) ) );
    m_seriesMap[ md.key ].series->setProperty( "_uniScale",
        metricToNormalisedScale( md.group ) );

    // Respect the toggle checkbox (connect and sync initial state)
    connect( m_seriesMap[ md.key ].toggle, &QCheckBox::toggled,
             ns, &QLineSeries::setVisible );
    ns->setVisible( m_seriesMap[ md.key ].toggle->isChecked() );

    // Copy existing raw data into the shadow series (normalised)
    // so the unified chart is immediately populated with all visible history.
    auto &rawBuffer = m_seriesMap[ md.key ].buffer;
    const double scale = metricToNormalisedScale( md.group );
    if ( !rawBuffer.isEmpty() )
    {
      QList< QPointF > pts;
      pts.reserve( rawBuffer.size() );
      for ( const auto &pt : rawBuffer )
        pts.append( QPointF( pt.x(), pt.y() * scale ) );
      ns->replace( pts );
    }
  }

  m_unifiedSeriesActive = true;
}

void MonitorTab::destroyUnifiedSeries()
{
  if ( !m_unifiedSeriesActive )
    return;  // Already destroyed

  // Remove all shadow series from the unified chart (keep invisible anchor).
  // The anchor has no name and is not visible; shadow series always have names.
  const auto allSeries = m_unifiedChart->series();   // snapshot (QList copy)
  for ( auto *abstractSeries : allSeries )
  {
    if ( !abstractSeries->isVisible() && abstractSeries->name().isEmpty() )
      continue;   // invisible anchor - keep
    m_unifiedChart->removeSeries( abstractSeries );
    delete abstractSeries;
  }

  // Clear the property references
  for ( auto &[key, info] : m_seriesMap )
  {
    info.series->setProperty( "_uniSeries", QVariant() );
    info.series->setProperty( "_uniScale", QVariant() );
  }

  m_unifiedSeriesActive = false;
}

void MonitorTab::setUnifiedMode( bool unified )
{
  if ( unified && !m_unifiedSeriesActive )
  {
    // Lazily create shadow series on first activation
    createUnifiedSeries();
    // Install hover callout now that the series actually exist
    installHoverCallout( m_unifiedChart );
    // Create unified-side sticky mark graphics for existing marks
    createUnifiedMarkGfx();
  }
  else if ( !unified && m_unifiedSeriesActive )
  {
    // Reclaim memory when switching back to per-group view
    destroyUnifiedMarkGfx();
    destroyUnifiedSeries();
  }
  m_chartStack->setCurrentIndex( unified ? 1 : 0 );
  updateAxes();
  updateStickyMarkPositions();
}

// Hover callout - shows exact value under the pointer

void MonitorTab::installHoverCallout( QChart *chart )
{
  // Clean up any previous callout for this chart (important for the
  // unified chart whose series are destroyed and recreated).
  auto it = m_callouts.find( chart );
  if ( it != m_callouts.end() )
  {
    delete it->second.bg;
    delete it->second.text;
    m_callouts.erase( it );
  }

  // Create a semi-transparent background rect + text item living in the
  // chart's scene. Hidden by default; shown on hover.
  auto *bg   = new QGraphicsRectItem( chart );
  auto *text = new QGraphicsSimpleTextItem( chart );
  bg->setBrush( QBrush( FluentTheme::colors().inset ) );
  bg->setPen( QPen( FluentTheme::colors().border ) );
  bg->setZValue( 100 );
  text->setBrush( FluentTheme::colors().text );
  text->setZValue( 101 );
  bg->hide();
  text->hide();

  m_callouts[ chart ] = { bg, text };

  // Connect hovered() on every series belonging to this chart.
  for ( auto *abstractSeries : chart->series() )
  {
    auto *ls = qobject_cast< QLineSeries * >( abstractSeries );
    if ( !ls )
      continue;

    connect( ls, &QLineSeries::hovered,
             this, [this, ls, chart]( const QPointF &point, bool state )
    {
      auto &co = m_callouts[ chart ];
      if ( !state )
      {
        co.bg->hide();
        co.text->hide();
        return;
      }

      // Format the timestamp and value.
      // _realScale is set on unified shadow series to invert the normalisation;
      // _unit is set on all series for the physical unit suffix.
      const QDateTime dt = QDateTime::fromMSecsSinceEpoch(
                              static_cast< qint64 >( point.x() ) );
      const QVariant rvProp = ls->property( "_realScale" );
      const double realVal  = rvProp.isValid()
                              ? point.y() * rvProp.toDouble()
                              : point.y();
      const QString unit    = ls->property( "_unit" ).toString();

      const QString label = unit.isEmpty()
          ? QStringLiteral( "%1\n%2: %3" )
              .arg( dt.toString( "HH:mm:ss" ) )
              .arg( ls->name() )
              .arg( realVal, 0, 'f', 1 )
          : QStringLiteral( "%1\n%2: %3 %4" )
              .arg( dt.toString( "HH:mm:ss" ) )
              .arg( ls->name() )
              .arg( realVal, 0, 'f', 1 )
              .arg( unit );

      co.text->setText( label );

      // Position near the data point (in scene coordinates)
      const QPointF scenePos = chart->mapToPosition( point );
      constexpr qreal pad = 4.0;
      const QRectF textRect = co.text->boundingRect();
      co.text->setPos( scenePos.x() + 10, scenePos.y() - textRect.height() - 6 );
      co.bg->setRect( co.text->pos().x() - pad,
                      co.text->pos().y() - pad,
                      textRect.width() + 2 * pad,
                      textRect.height() + 2 * pad );

      co.bg->show();
      co.text->show();
    } );

    // Sticky mark - click to pin/unpin a data-point label
    connect( ls, &QLineSeries::clicked,
             this, [this, ls]( const QPointF &point )
    {
      handleSeriesClick( ls, point );
    } );
  }
}

// Sticky marks - click to pin/unpin a data-point callout

int MonitorTab::metricIndexForKey( const std::string &key )
{
  for ( int i = 0; i < METRIC_COUNT; ++i )
  {
    if ( kMetrics[ i ].key == key )
      return i;
  }
  return -1;
}

QChart *MonitorTab::chartForGroup( MetricGroup g ) const
{
  switch ( g )
  {
    case MetricGroup::Temp:  return m_tempChart;
    case MetricGroup::Duty:  return m_dutyChart;
    case MetricGroup::Power: return m_powerChart;
    case MetricGroup::Freq:  return m_freqChart;
    case MetricGroup::Volt:  return m_voltChart;
  }
  return m_tempChart;
}

MonitorTab::MarkGfx MonitorTab::createMarkGfx( QChart *chart, const QColor &borderColor,
                                               std::function< void() > onClick )
{
  auto *bg   = new ClickableRectItem( chart );
  auto *text = new QGraphicsSimpleTextItem( chart );

  bg->setBrush( QBrush( FluentTheme::colors().inset ) );
  bg->setPen( QPen( borderColor, 2 ) );
  bg->setZValue( 90 );
  if ( onClick )
    bg->setClickCallback( std::move( onClick ) );

  // Let mouse clicks pass through the text to the bg rect behind it
  text->setAcceptedMouseButtons( Qt::NoButton );
  text->setBrush( FluentTheme::colors().text );
  text->setZValue( 91 );
  bg->hide();
  text->hide();

  return { bg, text };
}

void MonitorTab::positionMarkGfx( MarkGfx &gfx, QChart *chart,
                                  const QPointF &dataPoint, const QString &label )
{
  if ( !gfx.bg || !gfx.text )
    return;

  const QRectF plotArea = chart->plotArea();
  const QPointF scenePos = chart->mapToPosition( dataPoint );

  // Hide if the mark has scrolled outside the visible plot area
  if ( scenePos.x() < plotArea.left() || scenePos.x() > plotArea.right() )
  {
    gfx.bg->hide();
    gfx.text->hide();
    return;
  }

  gfx.text->setText( label );

  constexpr qreal pad = 4.0;
  const QRectF textRect = gfx.text->boundingRect();

  // Position above and to the right of the data point
  qreal tx = scenePos.x() + 10;
  qreal ty = scenePos.y() - textRect.height() - 10;

  // If it would overflow the right edge, flip to the left side
  if ( tx + textRect.width() + 2 * pad > plotArea.right() )
    tx = scenePos.x() - textRect.width() - 2 * pad - 10;

  // If it would overflow the top edge, flip below
  if ( ty - pad < plotArea.top() )
    ty = scenePos.y() + 10;

  gfx.text->setPos( tx, ty );
  gfx.bg->setRect( tx - pad, ty - pad,
                    textRect.width() + 2 * pad,
                    textRect.height() + 2 * pad );

  gfx.bg->show();
  gfx.text->show();
}

void MonitorTab::handleSeriesClick( QLineSeries *ls, const QPointF &point )
{
  const QString keyStr = ls->property( "_metricKey" ).toString();
  if ( keyStr.isEmpty() )
    return;

  const std::string key = keyStr.toStdString();
  const int idx = metricIndexForKey( key );
  if ( idx < 0 )
    return;

  // Determine raw value - denormalize if this is a unified shadow series
  const QVariant rvProp = ls->property( "_realScale" );
  const double rawValue = rvProp.isValid()
                          ? point.y() * rvProp.toDouble()
                          : point.y();

  const qint64 clickTs = static_cast< qint64 >( point.x() );

  if ( static_cast< int >( m_stickyMarks.size() ) >= MAX_STICKY_MARKS )
    return;

  // Snap to the nearest actual data point in the raw buffer
  auto &buf = m_seriesMap[ key ].buffer;
  if ( buf.isEmpty() )
    return;

  qint64 snapTs  = clickTs;
  double snapVal = rawValue;
  qint64 bestDist = m_windowSeconds * 1000LL + 1;

  for ( const auto &pt : buf )
  {
    const qint64 d = std::abs( static_cast< qint64 >( pt.x() ) - clickTs );
    if ( d < bestDist )
    {
      bestDist = d;
      snapTs  = static_cast< qint64 >( pt.x() );
      snapVal = pt.y();
    }
  }

  // Create a single-metric group mark - place box at vertical centre of plot
  addStickyMarkGroup( snapTs, 0.5, { { key, snapVal } } );
}

void MonitorTab::addStickyMarkGroup( qint64 ts, double clickDataY,
                                     const std::vector< StickyMetricEntry > &entries )
{
  StickyMark mark;
  mark.timestamp  = ts;
  mark.clickDataY = clickDataY;
  mark.entries    = entries;

  // Build a remove callback
  const qint64 capturedTs = ts;
  QPointer< MonitorTab > self( this );
  auto removeCb = [self, capturedTs]() {
    if ( !self )
      return;
    for ( auto it = self->m_stickyMarks.begin(); it != self->m_stickyMarks.end(); ++it )
    {
      if ( it->timestamp == capturedTs )
      {
        self->removeStickyMark( it );
        return;
      }
    }
  };

  // Create per-group chart graphics (one MarkGfx per entry)
  for ( const auto &entry : entries )
  {
    const int idx = metricIndexForKey( entry.metricKey );
    if ( idx < 0 )
    {
      mark.groupGfxList.push_back( {} );
      continue;
    }
    const auto &md = kMetrics[ idx ];
    mark.groupGfxList.push_back( createMarkGfx( chartForGroup( md.group ), metricColor(md), removeCb ) );
  }

  // Create unified chart graphics if the unified view is active
  if ( m_unifiedSeriesActive && m_unifiedChart )
  {
    // Background rect (clickable)
    auto *bg = new ClickableRectItem( m_unifiedChart );
    bg->setBrush( QBrush( FluentTheme::colors().inset ) );
    bg->setPen( QPen( FluentTheme::colors().border, 1 ) );
    bg->setZValue( 90 );
    bg->setClickCallback( removeCb );
    mark.uniBg = bg;

    // One text item per entry + timestamp header
    auto *tsText = new QGraphicsSimpleTextItem( m_unifiedChart );
    tsText->setBrush( FluentTheme::colors().text );
    tsText->setZValue( 91 );
    tsText->setAcceptedMouseButtons( Qt::NoButton );
    mark.uniTexts.push_back( tsText );

    for ( const auto &entry : entries )
    {
      const int idx = metricIndexForKey( entry.metricKey );
      const QColor col = ( idx >= 0 ) ? metricColor(kMetrics[ idx ]) : FluentTheme::colors().text;
      auto *txt = new QGraphicsSimpleTextItem( m_unifiedChart );
      txt->setBrush( col );
      txt->setZValue( 91 );
      txt->setAcceptedMouseButtons( Qt::NoButton );
      mark.uniTexts.push_back( txt );
    }

    // Vertical marker line
    auto *line = new QGraphicsLineItem( m_unifiedChart );
    line->setPen( QPen( QColor( 200, 200, 200, 150 ), 1, Qt::DashLine ) );
    line->setZValue( 89 );
    mark.uniLine = line;
  }

  m_stickyMarks.push_back( std::move( mark ) );
  updateStickyMarkPositions();
}

void MonitorTab::removeStickyMark( std::vector< StickyMark >::iterator it )
{
  // Per-group graphics
  for ( auto &gfx : it->groupGfxList )
  {
    delete gfx.bg;
    delete gfx.text;
  }

  // Unified graphics
  for ( auto *txt : it->uniTexts )
    delete txt;
  delete it->uniBg;
  delete it->uniLine;

  m_stickyMarks.erase( it );
}

void MonitorTab::updateStickyMarkPositions()
{
  for ( auto &mark : m_stickyMarks )
  {
    // Per-group chart positioning (one per entry)
    for ( size_t e = 0; e < mark.entries.size(); ++e )
    {
      if ( e >= mark.groupGfxList.size() )
        break;

      const auto &entry = mark.entries[ e ];
      auto &gfx = mark.groupGfxList[ e ];

      const int idx = metricIndexForKey( entry.metricKey );
      if ( idx < 0 )
        continue;

      const auto &md = kMetrics[ idx ];
      auto it = m_seriesMap.find( md.key );
      const bool visible = ( it != m_seriesMap.end() && it->second.toggle->isChecked() );

      if ( !visible )
      {
        if ( gfx.bg ) { gfx.bg->hide(); gfx.text->hide(); }
        continue;
      }

      const QDateTime dt = QDateTime::fromMSecsSinceEpoch( mark.timestamp );
      const QString unit = QString::fromUtf8( metricGroupUnit( md.group ) );
      const QString label = QStringLiteral( "%1\n%2: %3 %4" )
          .arg( dt.toString( "HH:mm:ss" ) )
          .arg( md.label )
          .arg( entry.rawValue, 0, 'f', 1 )
          .arg( unit );

      positionMarkGfx( gfx, chartForGroup( md.group ),
          QPointF( static_cast< qreal >( mark.timestamp ), entry.rawValue ), label );
    }

    // Unified chart grouped label
    if ( !mark.uniBg || !m_unifiedChart )
      continue;

    // Check if any entry is visible
    bool anyVisible = false;
    for ( const auto &entry : mark.entries )
    {
      const int idx = metricIndexForKey( entry.metricKey );
      if ( idx >= 0 )
      {
        auto it = m_seriesMap.find( entry.metricKey );
        if ( it != m_seriesMap.end() && it->second.toggle->isChecked() )
        {
          anyVisible = true;
          break;
        }
      }
    }

    const QRectF plotArea = m_unifiedChart->plotArea();
    const QPointF sceneX = m_unifiedChart->mapToPosition(
        QPointF( static_cast< qreal >( mark.timestamp ), 0 ) );

    if ( !anyVisible || sceneX.x() < plotArea.left() || sceneX.x() > plotArea.right() )
    {
      mark.uniBg->hide();
      for ( auto *txt : mark.uniTexts )
        txt->hide();
      if ( mark.uniLine )
        mark.uniLine->hide();
      continue;
    }

    // Set timestamp header text
    const QDateTime dt = QDateTime::fromMSecsSinceEpoch( mark.timestamp );
    mark.uniTexts[ 0 ]->setText( dt.toString( "HH:mm:ss" ) );

    // Set per-metric texts, build up total height
    constexpr qreal pad = 4.0;
    constexpr qreal rowGap = 1.0;
    qreal totalH = 0;
    qreal maxW   = 0;

    // Measure timestamp row
    {
      const QRectF r = mark.uniTexts[ 0 ]->boundingRect();
      totalH += r.height() + rowGap;
      maxW = std::max( maxW, r.width() );
    }

    // Measure + set metric rows
    size_t txtIdx = 1;
    for ( const auto &entry : mark.entries )
    {
      if ( txtIdx >= mark.uniTexts.size() )
        break;

      const int idx = metricIndexForKey( entry.metricKey );
      if ( idx < 0 ) { ++txtIdx; continue; }

      const auto &md = kMetrics[ idx ];
      auto it = m_seriesMap.find( md.key );
      const bool vis = ( it != m_seriesMap.end() && it->second.toggle->isChecked() );

      if ( !vis )
      {
        mark.uniTexts[ txtIdx ]->hide();
        ++txtIdx;
        continue;
      }

      const QString unit = QString::fromUtf8( metricGroupUnit( md.group ) );
      const QString rowText = QStringLiteral( "%1: %2 %3" )
          .arg( md.label )
          .arg( entry.rawValue, 0, 'f', 1 )
          .arg( unit );

      mark.uniTexts[ txtIdx ]->setText( rowText );
      mark.uniTexts[ txtIdx ]->show();

      const QRectF r = mark.uniTexts[ txtIdx ]->boundingRect();
      totalH += r.height() + rowGap;
      maxW = std::max( maxW, r.width() );
      ++txtIdx;
    }

    // Position the label box above the data, anchored at sceneX
    const qreal boxW = maxW + 2 * pad;
    const qreal boxH = totalH + 2 * pad - rowGap;  // remove last rowGap

    qreal bx = sceneX.x() + 8;
    if ( bx + boxW > plotArea.right() )
      bx = sceneX.x() - boxW - 8;

    // Place box at the stored plot-fraction Y - independent of axis zoom
    qreal by = plotArea.top() + mark.clickDataY * plotArea.height() - boxH / 2.0;
    by = std::max( plotArea.top() + 2.0, by );
    by = std::min( plotArea.bottom() - boxH - 2.0, by );

    mark.uniBg->setRect( bx, by, boxW, boxH );
    mark.uniBg->show();

    // Position text rows inside the box
    qreal rowY = by + pad;
    for ( size_t t = 0; t < mark.uniTexts.size(); ++t )
    {
      auto *txt = mark.uniTexts[ t ];
      if ( !txt->isVisible() && t != 0 )
        continue;

      if ( t == 0 )
        txt->show();

      txt->setPos( bx + pad, rowY );
      rowY += txt->boundingRect().height() + rowGap;
    }

    // Vertical line from bottom of box to X axis
    if ( mark.uniLine )
    {
      mark.uniLine->setLine( sceneX.x(), plotArea.top(), sceneX.x(), plotArea.bottom() );
      mark.uniLine->show();
    }
  }
}

void MonitorTab::createUnifiedMarkGfx()
{
  for ( auto &mark : m_stickyMarks )
  {
    if ( mark.uniBg )
      continue;  // Already has unified graphics

    const qint64 capturedTs = mark.timestamp;
    QPointer< MonitorTab > self( this );
    auto removeCb = [self, capturedTs]() {
      if ( !self )
        return;
      for ( auto it = self->m_stickyMarks.begin(); it != self->m_stickyMarks.end(); ++it )
      {
        if ( it->timestamp == capturedTs )
        {
          self->removeStickyMark( it );
          return;
        }
      }
    };

    auto *bg = new ClickableRectItem( m_unifiedChart );
    bg->setBrush( QBrush( FluentTheme::colors().inset ) );
    bg->setPen( QPen( FluentTheme::colors().border, 1 ) );
    bg->setZValue( 90 );
    bg->setClickCallback( removeCb );
    mark.uniBg = bg;

    // Timestamp text
    auto *tsText = new QGraphicsSimpleTextItem( m_unifiedChart );
    tsText->setBrush( FluentTheme::colors().text );
    tsText->setZValue( 91 );
    tsText->setAcceptedMouseButtons( Qt::NoButton );
    mark.uniTexts.push_back( tsText );

    // Per-metric texts
    for ( const auto &entry : mark.entries )
    {
      const int idx = metricIndexForKey( entry.metricKey );
      const QColor col = ( idx >= 0 ) ? metricColor(kMetrics[ idx ]) : FluentTheme::colors().text;
      auto *txt = new QGraphicsSimpleTextItem( m_unifiedChart );
      txt->setBrush( col );
      txt->setZValue( 91 );
      txt->setAcceptedMouseButtons( Qt::NoButton );
      mark.uniTexts.push_back( txt );
    }

    auto *line = new QGraphicsLineItem( m_unifiedChart );
    line->setPen( QPen( QColor( 200, 200, 200, 150 ), 1, Qt::DashLine ) );
    line->setZValue( 89 );
    mark.uniLine = line;
  }
}

void MonitorTab::destroyUnifiedMarkGfx()
{
  for ( auto &mark : m_stickyMarks )
  {
    for ( auto *txt : mark.uniTexts )
      delete txt;
    mark.uniTexts.clear();
    delete mark.uniBg;
    mark.uniBg = nullptr;
    delete mark.uniLine;
    mark.uniLine = nullptr;
  }
}

// Unified crosshair - vertical line + per-series labels on Ctrl

bool MonitorTab::eventFilter( QObject *watched, QEvent *event )
{
  // Only handle mouse events on the unified chart viewport
  if ( watched != m_unifiedChartView->viewport() )
    return QWidget::eventFilter( watched, event );

  switch ( event->type() )
  {
    case QEvent::MouseMove:
    {
      auto *me = static_cast< QMouseEvent * >( event );

      // Rubber-band zoom drag in progress
      if ( m_zoomDragging )
      {
        m_zoomBand->setGeometry( QRect( m_zoomOrigin, me->pos() ).normalized() );
        return true;
      }

      m_lastCrosshairPos = me->pos();
      m_cursorInPlot = true;
      updateCrosshair( me->pos(), m_annotationsVisible );
      break;
    }
    case QEvent::MouseButtonPress:
    {
      auto *me = static_cast< QMouseEvent * >( event );

      // Ctrl+LMB starts rubber-band zoom
      if ( me->button() == Qt::LeftButton
           && ( me->modifiers() & Qt::ControlModifier ) )
      {
        m_zoomOrigin = me->pos();
        if ( !m_zoomBand )
          m_zoomBand = new QRubberBand( QRubberBand::Rectangle, m_unifiedChartView->viewport() );
        m_zoomBand->setGeometry( QRect( m_zoomOrigin, QSize() ) );
        m_zoomBand->show();
        m_zoomDragging = true;
        return true;
      }

      if ( me->button() == Qt::LeftButton && m_annotationsVisible )
      {
        // Check if the click is on a sticky mark graphics item
        const QPointF scenePos = m_unifiedChartView->mapToScene(
            static_cast< int >( me->pos().x() ),
            static_cast< int >( me->pos().y() ) );
        const auto items = m_unifiedChart->scene()->items( scenePos );
        
        // If a graphics item was clicked (mark bg rect), let it through
        for ( auto *item : items )
        {
          if ( dynamic_cast< ClickableRectItem * >( item ) )
            return false;  // Don't consume; let the item handle it
        }
        
        // No mark clicked; create a new one on empty space
        crosshairClick( me->pos() );
        return true;
      }
      break;
    }
    case QEvent::MouseButtonRelease:
    {
      auto *me = static_cast< QMouseEvent * >( event );

      // Finish rubber-band zoom
      if ( me->button() == Qt::LeftButton && m_zoomDragging )
      {
        m_zoomDragging = false;
        m_zoomBand->hide();
        const QRect rect = QRect( m_zoomOrigin, me->pos() ).normalized();
        if ( rect.width() > 4 && rect.height() > 4 )
          applyZoomRect( rect );
        return true;
      }

      if ( me->button() == Qt::RightButton )
      {
        m_annotationsVisible = !m_annotationsVisible;
        updateCrosshair( me->pos(), m_annotationsVisible );
        return true;  // suppress context menu
      }
      break;
    }
    case QEvent::Leave:
      m_cursorInPlot = false;
      hideCrosshair();
      break;
    default:
      break;
  }

  return QWidget::eventFilter( watched, event );
}

void MonitorTab::hideCrosshair()
{
  if ( m_crosshairLine )
    m_crosshairLine->hide();

  for ( auto &cl : m_crosshairLabels )
  {
    delete cl.bg;
    delete cl.text;
  }
  m_crosshairLabels.clear();
  m_crosshairVisible = false;
}

void MonitorTab::updateCrosshair( const QPointF &widgetPos, bool ctrlHeld )
{
  if ( !m_unifiedSeriesActive || !m_unifiedChart )
  {
    hideCrosshair();
    return;
  }

  // Map the widget position to chart coordinates.
  // The viewport() pos maps to the QGraphicsView, which we convert to scene
  // and then to chart-value coordinates.
  const QPointF scenePos = m_unifiedChartView->mapToScene(
      static_cast< int >( widgetPos.x() ),
      static_cast< int >( widgetPos.y() ) );
  const QRectF plotArea = m_unifiedChart->plotArea();

  if ( !plotArea.contains( scenePos ) )
  {
    hideCrosshair();
    return;
  }

  // Draw the vertical crosshair line spanning the full plot height
  m_crosshairLine->setLine( scenePos.x(), plotArea.top(),
                             scenePos.x(), plotArea.bottom() );
  m_crosshairLine->show();

  // Clean up old labels
  for ( auto &cl : m_crosshairLabels )
  {
    delete cl.bg;
    delete cl.text;
  }
  m_crosshairLabels.clear();

  if ( !ctrlHeld )
  {
    m_crosshairVisible = true;
    return;
  }

  // Map scenePos to a data-space X (timestamp)
  const QPointF dataPos = m_unifiedChart->mapToValue( scenePos );
  const qint64 cursorTs = static_cast< qint64 >( dataPos.x() );

  // Pre-count visible metrics so we can center the label stack on the cursor Y
  int totalLabels = 1; // +1 for the timestamp row
  for ( int i = 0; i < METRIC_COUNT; ++i )
  {
    const auto &md  = kMetrics[ i ];
    const auto &info = m_seriesMap[ md.key ];
    if ( info.toggle->isChecked() && !info.buffer.isEmpty() )
      ++totalLabels;
  }

  // For each visible metric, find the nearest data point and create a label
  int labelIndex = 0;
  for ( int i = 0; i < METRIC_COUNT; ++i )
  {
    const auto &md  = kMetrics[ i ];
    auto &info = m_seriesMap[ md.key ];
    if ( !info.toggle->isChecked() )
      continue;

    const auto &buf = info.buffer;
    if ( buf.isEmpty() )
      continue;

    // Binary search for the nearest timestamp in the raw buffer
    int lo = 0, hi = buf.size() - 1;
    while ( lo < hi )
    {
      const int mid = lo + ( hi - lo ) / 2;
      if ( static_cast< qint64 >( buf[ mid ].x() ) < cursorTs )
        lo = mid + 1;
      else
        hi = mid;
    }

    // Check the neighbor as well to find the truly closest
    int bestIdx = lo;
    if ( lo > 0 )
    {
      const qint64 dLo = std::abs( static_cast< qint64 >( buf[ lo ].x() ) - cursorTs );
      const qint64 dPrev = std::abs( static_cast< qint64 >( buf[ lo - 1 ].x() ) - cursorTs );
      if ( dPrev < dLo )
        bestIdx = lo - 1;
    }

    const double rawVal = buf[ bestIdx ].y();
    const qint64 snapTs = static_cast< qint64 >( buf[ bestIdx ].x() );

    // Compute the normalised data point for potential future use
    const double scale = metricToNormalisedScale( md.group );
    (void) scale; // Used below for mapToPosition if needed

    // Build label text
    const QDateTime dt = QDateTime::fromMSecsSinceEpoch( snapTs );
    const QString unit = QString::fromUtf8( metricGroupUnit( md.group ) );
    const QString label = QStringLiteral( "%1: %2 %3" )
        .arg( md.label )
        .arg( rawVal, 0, 'f', 1 )
        .arg( unit );

    // Create graphics items
    auto *bg   = new QGraphicsRectItem( m_unifiedChart );
    auto *text = new QGraphicsSimpleTextItem( m_unifiedChart );

    bg->setBrush( QBrush( FluentTheme::colors().inset ) );
    bg->setPen( QPen( metricColor(md), 1 ) );
    bg->setZValue( 95 );
    text->setBrush( metricColor(md) );
    text->setZValue( 96 );
    text->setAcceptedMouseButtons( Qt::NoButton );

    text->setText( label );

    constexpr qreal pad = 3.0;
    const QRectF textRect = text->boundingRect();

    // Stack labels centered on the cursor Y
    const qreal rowH   = textRect.height() + 2 * pad + 2;
    const qreal startY = std::max( plotArea.top() + 2,
                           std::min( scenePos.y() - totalLabels * rowH / 2.0,
                                     plotArea.bottom() - totalLabels * rowH - 2 ) );
    const qreal baseY  = startY + labelIndex * rowH;
    qreal tx = scenePos.x() + 12;

    // If it would overflow the right edge, flip to the left
    if ( tx + textRect.width() + 2 * pad > plotArea.right() )
      tx = scenePos.x() - textRect.width() - 2 * pad - 12;

    text->setPos( tx, baseY );
    bg->setRect( tx - pad, baseY - pad,
                 textRect.width() + 2 * pad,
                 textRect.height() + 2 * pad );

    bg->show();
    text->show();

    m_crosshairLabels.push_back( { bg, text } );
    ++labelIndex;
  }

  // Time label - always shown at the bottom of the stack (white)
  {
    constexpr qreal pad = 3.0;
    auto *bg   = new QGraphicsRectItem( m_unifiedChart );
    auto *text = new QGraphicsSimpleTextItem( m_unifiedChart );

    bg->setBrush( QBrush( FluentTheme::colors().inset ) );
    bg->setPen( QPen( FluentTheme::colors().border, 1 ) );
    bg->setZValue( 95 );
    text->setBrush( FluentTheme::colors().text );
    text->setZValue( 96 );
    text->setAcceptedMouseButtons( Qt::NoButton );

    const QDateTime dt = QDateTime::fromMSecsSinceEpoch( cursorTs );
    text->setText( dt.toString( "HH:mm:ss" ) );

    const QRectF textRect = text->boundingRect();
    const qreal rowH   = textRect.height() + 2 * pad + 2;
    const qreal startY = std::max( plotArea.top() + 2,
                           std::min( scenePos.y() - totalLabels * rowH / 2.0,
                                     plotArea.bottom() - totalLabels * rowH - 2 ) );
    const qreal baseY  = startY + labelIndex * rowH;
    qreal tx = scenePos.x() + 12;
    if ( tx + textRect.width() + 2 * pad > plotArea.right() )
      tx = scenePos.x() - textRect.width() - 2 * pad - 12;

    text->setPos( tx, baseY );
    bg->setRect( tx - pad, baseY - pad,
                 textRect.width() + 2 * pad,
                 textRect.height() + 2 * pad );
    bg->show();
    text->show();

    m_crosshairLabels.push_back( { bg, text } );
  }

  m_crosshairVisible = true;
}

void MonitorTab::crosshairClick( const QPointF &widgetPos )
{
  if ( !m_unifiedSeriesActive || !m_unifiedChart )
    return;

  const QPointF scenePos = m_unifiedChartView->mapToScene(
      static_cast< int >( widgetPos.x() ),
      static_cast< int >( widgetPos.y() ) );
  const QRectF plotArea = m_unifiedChart->plotArea();

  if ( !plotArea.contains( scenePos ) )
    return;

  const QPointF dataPos = m_unifiedChart->mapToValue( scenePos );
  const qint64 cursorTs = static_cast< qint64 >( dataPos.x() );

  if ( static_cast< int >( m_stickyMarks.size() ) >= MAX_STICKY_MARKS )
    return;

  // Collect all visible metrics at this timestamp
  std::vector< StickyMetricEntry > entries;
  qint64 snapTs = cursorTs;

  for ( int i = 0; i < METRIC_COUNT; ++i )
  {
    const auto &md  = kMetrics[ i ];
    auto &info = m_seriesMap[ md.key ];
    if ( !info.toggle->isChecked() )
      continue;

    const auto &buf = info.buffer;
    if ( buf.isEmpty() )
      continue;

    // Binary search for the nearest timestamp
    int lo = 0, hi = buf.size() - 1;
    while ( lo < hi )
    {
      const int mid = lo + ( hi - lo ) / 2;
      if ( static_cast< qint64 >( buf[ mid ].x() ) < cursorTs )
        lo = mid + 1;
      else
        hi = mid;
    }

    int bestIdx = lo;
    if ( lo > 0 )
    {
      const qint64 dLo = std::abs( static_cast< qint64 >( buf[ lo ].x() ) - cursorTs );
      const qint64 dPrev = std::abs( static_cast< qint64 >( buf[ lo - 1 ].x() ) - cursorTs );
      if ( dPrev < dLo )
        bestIdx = lo - 1;
    }

    // Use the first metric's snapped timestamp as the group timestamp
    if ( entries.empty() )
      snapTs = static_cast< qint64 >( buf[ bestIdx ].x() );

    entries.push_back( { md.key, buf[ bestIdx ].y() } );
  }

  if ( !entries.empty() )
  {
    const double plotFrac = ( plotArea.height() > 0 )
        ? ( scenePos.y() - plotArea.top() ) / plotArea.height()
        : 0.5;
    addStickyMarkGroup( snapTs, plotFrac, entries );
  }
}

// Ctrl+LMB rubber-band zoom

void MonitorTab::applyZoomRect( const QRect &viewportRect )
{
  if ( !m_unifiedChart || !m_unifiedXAxis || !m_unifiedYAxis )
    return;

  // Map viewport corners to scene, then to data coordinates
  const QPointF topLeft = m_unifiedChartView->mapToScene( viewportRect.topLeft() );
  const QPointF bottomRight = m_unifiedChartView->mapToScene( viewportRect.bottomRight() );

  const QRectF plotArea = m_unifiedChart->plotArea();

  // Clamp to the plot area
  const QPointF clampedTL(
      std::max( topLeft.x(), plotArea.left() ),
      std::max( topLeft.y(), plotArea.top() ) );
  const QPointF clampedBR(
      std::min( bottomRight.x(), plotArea.right() ),
      std::min( bottomRight.y(), plotArea.bottom() ) );

  const QPointF dataMin = m_unifiedChart->mapToValue( clampedTL );
  const QPointF dataMax = m_unifiedChart->mapToValue( clampedBR );

  // dataMin.y() is the top of the rect -> higher value (Y axis is inverted in screen coords)
  // dataMax.y() is the bottom of the rect -> lower value
  const qreal yLo = std::min( dataMin.y(), dataMax.y() );
  const qreal yHi = std::max( dataMin.y(), dataMax.y() );
  const qint64 tLo = static_cast< qint64 >( std::min( dataMin.x(), dataMax.x() ) );
  const qint64 tHi = static_cast< qint64 >( std::max( dataMin.x(), dataMax.x() ) );

  // Pause data fetching
  m_paused = true;
  syncFrameTimer();
  updateMonitorStatus();

  // Apply zoomed ranges
  m_unifiedXAxis->setRange(
      QDateTime::fromMSecsSinceEpoch( tLo ),
      QDateTime::fromMSecsSinceEpoch( tHi ) );
  m_unifiedYAxis->setRange( yLo, yHi );

  m_zoomed = true;
  updateStickyMarkPositions();
}

void MonitorTab::resetZoom()
{
  if ( !m_unifiedYAxis )
    return;

  m_unifiedYAxis->setRange( 0, 100 );
  m_zoomed = false;
  updateStickyMarkPositions();
  // The X axis will be restored by updateAxes() on the next tick
}

// Chart setup helpers - attach series to charts

void MonitorTab::setupTemperatureChart()
{
  m_tempChart = createChart();
  m_tempXAxis = createXAxis();
  m_tempYAxis = createYAxis( "Temperature (°C)", 0, 105 );
  m_tempChart->addAxis( m_tempXAxis, Qt::AlignBottom );
  m_tempChart->addAxis( m_tempYAxis, Qt::AlignLeft );

  for ( const auto &md : kMetrics )
  {
    if ( md.group != MetricGroup::Temp ) continue;
    auto *s = m_seriesMap[ md.key ].series;
    m_tempChart->addSeries( s );
    s->attachAxis( m_tempXAxis );
    s->attachAxis( m_tempYAxis );
  }
  m_tempChartView = createChartView( m_tempChart );
}

void MonitorTab::setupDutyChart()
{
  m_dutyChart = createChart();
  m_dutyXAxis = createXAxis();
  m_dutyYAxis = createYAxis( "Fan Duty (%)", 0, 100 );
  m_dutyChart->addAxis( m_dutyXAxis, Qt::AlignBottom );
  m_dutyChart->addAxis( m_dutyYAxis, Qt::AlignLeft );

  for ( const auto &md : kMetrics )
  {
    if ( md.group != MetricGroup::Duty ) continue;
    auto *s = m_seriesMap[ md.key ].series;
    m_dutyChart->addSeries( s );
    s->attachAxis( m_dutyXAxis );
    s->attachAxis( m_dutyYAxis );
  }
  m_dutyChartView = createChartView( m_dutyChart );
}

void MonitorTab::setupPowerChart()
{
  m_powerChart = createChart();
  m_powerXAxis = createXAxis();
  m_powerYAxis = createYAxis( QStringLiteral( "Power (W)" ), 0, 250 );
  m_powerChart->addAxis( m_powerXAxis, Qt::AlignBottom );
  m_powerChart->addAxis( m_powerYAxis, Qt::AlignLeft );

  for ( const auto &md : kMetrics )
  {
    if ( md.group != MetricGroup::Power ) continue;
    auto *s = m_seriesMap[ md.key ].series;
    m_powerChart->addSeries( s );
    s->attachAxis( m_powerXAxis );
    s->attachAxis( m_powerYAxis );
  }
  m_powerChartView = createChartView( m_powerChart );
}

void MonitorTab::setupFrequencyChart()
{
  m_freqChart = createChart();
  m_freqXAxis = createXAxis();
  m_freqYAxis = createYAxis( "Frequency (MHz)", 0, 6000 );
  m_freqChart->addAxis( m_freqXAxis, Qt::AlignBottom );
  m_freqChart->addAxis( m_freqYAxis, Qt::AlignLeft );

  for ( const auto &md : kMetrics )
  {
    if ( md.group != MetricGroup::Freq ) continue;
    auto *s = m_seriesMap[ md.key ].series;
    m_freqChart->addSeries( s );
    s->attachAxis( m_freqXAxis );
    s->attachAxis( m_freqYAxis );
  }
  m_freqChartView = createChartView( m_freqChart );
}

void MonitorTab::setupVoltageChart()
{
  m_voltChart = createChart();
  m_voltXAxis = createXAxis();
  m_voltYAxis = createYAxis( "Core Voltage (mV)", 0, 1500 );
  m_voltChart->addAxis( m_voltXAxis, Qt::AlignBottom );
  m_voltChart->addAxis( m_voltYAxis, Qt::AlignLeft );

  for ( const auto &md : kMetrics )
  {
    if ( md.group != MetricGroup::Volt ) continue;
    auto *s = m_seriesMap[ md.key ].series;
    m_voltChart->addSeries( s );
    s->attachAxis( m_voltXAxis );
    s->attachAxis( m_voltYAxis );
  }
  m_voltChartView = createChartView( m_voltChart );
}

CpuCoreGridWidget::CpuCoreGridWidget( QWidget *parent )
  : QWidget( parent )
{
  setObjectName( QStringLiteral( "cpuCoreGridWidget" ) );
  setSizePolicy( QSizePolicy::Expanding, QSizePolicy::Minimum );
  setMinimumHeight( 72 );
}

void CpuCoreGridWidget::setTopology( const QMap< int, QString > &coreTypes )
{
  m_coreTypes = coreTypes;
  const int w = width() > 0 ? width() : 640;
  m_lastColumns = columnsForWidth( w );
  setMinimumHeight( computeContentHeight( w ) );
  updateGeometry();
  update();
}

void CpuCoreGridWidget::setCoreLoads( const QVector< double > &loads )
{
  const bool countChanged = ( loads.size() != m_loads.size() );
  m_loads = loads;
  if ( countChanged )
  {
    const int w = width() > 0 ? width() : 640;
    m_lastColumns = columnsForWidth( w );
    setMinimumHeight( computeContentHeight( w ) );
    updateGeometry();
  }
  update();
}

void CpuCoreGridWidget::clearLoads()
{
  for ( double &v : m_loads )
    v = -1.0;
  update();
}

QVector< CpuCoreGridWidget::CoreGroup > CpuCoreGridWidget::buildGroups() const
{
  QVector< CoreGroup > groups;
  const int count = static_cast< int >( m_loads.size() );
  if ( count <= 0 )
    return groups;

  bool hasHybrid = false;
  for ( int id = 0; id < count; ++id )
  {
    const QString t = m_coreTypes.value( id );
    if ( t == QLatin1String( "P" ) || t == QLatin1String( "E" ) )
    {
      hasHybrid = true;
      break;
    }
  }

  if ( !hasHybrid )
  {
    CoreGroup all;
    all.title = QStringLiteral( "Cores" );
    all.coreIds.reserve( count );
    for ( int id = 0; id < count; ++id )
      all.coreIds.push_back( id );
    groups.push_back( std::move( all ) );
    return groups;
  }

  CoreGroup pGroup{ QStringLiteral( "P-cores" ), {} };
  CoreGroup eGroup{ QStringLiteral( "E-cores" ), {} };
  CoreGroup otherGroup{ QStringLiteral( "Other cores" ), {} };
  for ( int id = 0; id < count; ++id )
  {
    const QString t = m_coreTypes.value( id );
    if ( t == QLatin1String( "P" ) )
      pGroup.coreIds.push_back( id );
    else if ( t == QLatin1String( "E" ) )
      eGroup.coreIds.push_back( id );
    else
      otherGroup.coreIds.push_back( id );
  }
  if ( !pGroup.coreIds.isEmpty() )
    groups.push_back( std::move( pGroup ) );
  if ( !eGroup.coreIds.isEmpty() )
    groups.push_back( std::move( eGroup ) );
  if ( !otherGroup.coreIds.isEmpty() )
    groups.push_back( std::move( otherGroup ) );
  return groups;
}

int CpuCoreGridWidget::columnsForWidth( int w ) const
{
  if ( w >= 760 )
    return 4;
  if ( w >= 520 )
    return 3;
  return 2;
}

int CpuCoreGridWidget::computeContentHeight( int w ) const
{
  const auto groups = buildGroups();
  if ( groups.isEmpty() )
    return 56;
  const int cols = columnsForWidth( w > 0 ? w : 640 );
  constexpr int kHeaderH = 18;
  constexpr int kRowH = 22;
  constexpr int kRowGap = 4;
  constexpr int kGroupGap = 10;
  int totalH = 4;
  for ( qsizetype g = 0; g < groups.size(); ++g )
  {
    if ( g > 0 )
      totalH += kGroupGap;
    totalH += kHeaderH;
    const int rows = ( static_cast< int >( groups[g].coreIds.size() ) + cols - 1 ) / cols;
    totalH += rows * kRowH + std::max( 0, rows - 1 ) * kRowGap;
  }
  totalH += 4;
  return std::max( 56, totalH );
}

QSize CpuCoreGridWidget::sizeHint() const
{
  return QSize( 640, computeContentHeight( width() > 0 ? width() : 640 ) );
}

QSize CpuCoreGridWidget::minimumSizeHint() const
{
  return QSize( 320, computeContentHeight( width() > 0 ? width() : 480 ) );
}

void CpuCoreGridWidget::resizeEvent( QResizeEvent *event )
{
  QWidget::resizeEvent( event );
  const int w = event->size().width();
  const int cols = columnsForWidth( w );
  const int targetH = computeContentHeight( w );
  if ( cols != m_lastColumns || targetH != minimumHeight() )
  {
    m_lastColumns = cols;
    setMinimumHeight( targetH );
    updateGeometry();
  }
}

void CpuCoreGridWidget::paintEvent( QPaintEvent *event )
{
  Q_UNUSED( event );
  QPainter p( this );
  p.setRenderHint( QPainter::Antialiasing, true );

  const auto c = FluentTheme::colors();
  const bool dark = FluentTheme::isDark();
  const auto groups = buildGroups();
  if ( groups.isEmpty() )
  {
    p.setPen( c.secondary );
    p.drawText( rect(), Qt::AlignCenter, QStringLiteral( "Sampling CPU cores…" ) );
    return;
  }

  const QColor amber = dark ? QColor( "#F59E0B" ) : QColor( "#D97706" );
  const QColor crimson = dark ? QColor( "#F87171" ) : QColor( "#DC2626" );

  const int cols = columnsForWidth( width() );
  constexpr int kColGap = 14;
  constexpr int kHeaderH = 18;
  constexpr int kRowH = 22;
  constexpr int kRowGap = 4;
  constexpr int kGroupGap = 10;

  const int usableW = std::max( 1, width() - ( cols - 1 ) * kColGap );
  const int cellW = usableW / cols;

  QFont groupFont = font();
  groupFont.setPixelSize( 11 );
  groupFont.setWeight( QFont::DemiBold );

  QFont labelFont = font();
  labelFont.setPixelSize( 11 );
  labelFont.setStyleHint( QFont::Monospace );

  QFont badgeFont = font();
  badgeFont.setPixelSize( 9 );
  badgeFont.setWeight( QFont::Bold );

  QFont pctFont = labelFont;
  QFont pctBoldFont = labelFont;
  pctBoldFont.setWeight( QFont::Bold );

  int y = 2;
  for ( qsizetype g = 0; g < groups.size(); ++g )
  {
    if ( g > 0 )
      y += kGroupGap;

    const auto &group = groups[g];
    p.setFont( groupFont );
    p.setPen( c.secondary );
    p.drawText( QRect( 0, y, width(), kHeaderH ), Qt::AlignLeft | Qt::AlignVCenter, group.title );
    y += kHeaderH;

    const int n = static_cast< int >( group.coreIds.size() );
    const int rows = ( n + cols - 1 ) / cols;
    for ( int i = 0; i < n; ++i )
    {
      const int coreId = group.coreIds[i];
      const int col = i % cols;
      const int row = i / cols;
      const int x = col * ( cellW + kColGap );
      const int ry = y + row * ( kRowH + kRowGap );

      const double rawVal = ( coreId >= 0 && coreId < static_cast< int >( m_loads.size() ) )
        ? m_loads[coreId]
        : -1.0;
      const bool hasVal = ( rawVal >= 0.0 );
      const double pct = hasVal ? std::clamp( rawVal, 0.0, 100.0 ) : 0.0;
      const int pctInt = hasVal ? qRound( pct ) : -1;

      const QString coreType = m_coreTypes.value( coreId );
      const bool hasBadge = ( coreType == QLatin1String( "P" ) || coreType == QLatin1String( "E" ) );

      // Core ID label (e.g. C03)
      constexpr int kIdW = 28;
      constexpr int kBadgeW = 16;
      constexpr int kBadgeGap = 4;
      constexpr int kPctW = 34;
      const int leftBlockW = kIdW + ( hasBadge ? ( kBadgeGap + kBadgeW ) : 0 );

      p.setFont( labelFont );
      p.setPen( c.text );
      const QString idText = QStringLiteral( "C%1" ).arg( coreId, 2, 10, QLatin1Char( '0' ) );
      p.drawText( QRect( x, ry, kIdW, kRowH ), Qt::AlignLeft | Qt::AlignVCenter, idText );

      // Subtle P / E badge pill
      if ( hasBadge )
      {
        const QRect badgeRect( x + kIdW + kBadgeGap, ry + ( kRowH - 14 ) / 2, kBadgeW, 14 );
        QColor badgeBg = ( coreType == QLatin1String( "P" ) ) ? c.accent : c.secondary;
        badgeBg.setAlpha( dark ? 48 : 32 );
        p.setPen( Qt::NoPen );
        p.setBrush( badgeBg );
        p.drawRoundedRect( badgeRect, 3, 3 );
        p.setFont( badgeFont );
        p.setPen( ( coreType == QLatin1String( "P" ) ) ? c.accent : c.secondary );
        p.drawText( badgeRect, Qt::AlignCenter, coreType );
      }

      // Horizontal btop-style bar
      const int barX = x + leftBlockW + 6;
      const int barW = std::max( 24, cellW - leftBlockW - 6 - kPctW - 6 );
      const int barH = 10;
      const int barY = ry + ( kRowH - barH ) / 2;
      const QRectF trackRect( barX, barY, barW, barH );

      p.setPen( QPen( c.border, 1.0 ) );
      p.setBrush( c.inset );
      p.drawRoundedRect( trackRect, 3.0, 3.0 );

      const QColor fillColor = ( pct >= 90.0 ) ? crimson : ( pct >= 70.0 ? amber : c.accent );
      if ( hasVal && pct > 0.5 )
      {
        const qreal fillW = std::max< qreal >( 3.0, ( barW - 2.0 ) * ( pct / 100.0 ) );
        const QRectF fillRect( barX + 1.0, barY + 1.0, fillW, barH - 2.0 );
        p.setPen( Qt::NoPen );
        p.setBrush( fillColor );
        p.drawRoundedRect( fillRect, 2.0, 2.0 );

        // Subtle segmented btop ticks
        QColor tickColor = c.surface;
        tickColor.setAlpha( dark ? 70 : 95 );
        p.setPen( QPen( tickColor, 1.0 ) );
        constexpr int kSegments = 10;
        for ( int seg = 1; seg < kSegments; ++seg )
        {
          const qreal tx = barX + 1.0 + ( ( barW - 2.0 ) * seg ) / kSegments;
          if ( tx < barX + 1.0 + fillW - 1.0 )
            p.drawLine( QPointF( tx, barY + 1.0 ), QPointF( tx, barY + barH - 1.0 ) );
        }
      }

      // Right-aligned percentage
      const QRect pctRect( barX + barW + 6, ry, kPctW, kRowH );
      p.setFont( pct >= 90.0 ? pctBoldFont : pctFont );
      p.setPen( !hasVal ? c.secondary : ( pct >= 90.0 ? crimson : ( pct >= 70.0 ? amber : c.text ) ) );
      const QString pctText = hasVal ? QStringLiteral( "%1%" ).arg( pctInt ) : QStringLiteral( "—%" );
      p.drawText( pctRect, Qt::AlignRight | Qt::AlignVCenter, pctText );
    }

    y += rows * kRowH + std::max( 0, rows - 1 ) * kRowGap;
  }
}

void MonitorTab::setupCoreLoadCard()
{
  m_coreLoadCard = FluentTheme::createCard();
  m_coreLoadCard->setObjectName( "cpuCoreLoadCard" );
  auto *layout = new QVBoxLayout( m_coreLoadCard );
  layout->setContentsMargins( 14, 12, 14, 12 );
  layout->setSpacing( 8 );

  auto *header = new QHBoxLayout;
  header->setSpacing( 12 );
  auto *heading = new QLabel( "CPU Core Load" );
  heading->setObjectName( "cardTitle" );
  header->addWidget( heading );
  header->addStretch();
  m_coreLoadSummary = new QLabel( "Sampling cores…" );
  m_coreLoadSummary->setObjectName( "cpuCoreLoadSummary" );
  m_coreLoadSummary->setStyleSheet( "font-size: 12px;" );
  header->addWidget( m_coreLoadSummary );
  layout->addLayout( header );

  m_coreLoadGrid = new CpuCoreGridWidget( m_coreLoadCard );
  layout->addWidget( m_coreLoadGrid );
}

void MonitorTab::fetchCpuTopology()
{
  readUccdAsync( this, QStringLiteral( "GetCpuTopologyJSON" ), {}, [this]( std::optional< QVariant > reply ) {
    if ( !reply )
      return;
    const QJsonArray arr = QJsonDocument::fromJson( reply->toString().toUtf8() ).array();
    QMap< int, QString > types;
    for ( const auto &val : arr )
    {
      const auto obj = val.toObject();
      const int id = obj.value( QStringLiteral( "id" ) ).toInt( -1 );
      const QString type = obj.value( QStringLiteral( "type" ) ).toString();
      if ( id >= 0 && !type.isEmpty() )
        types.insert( id, type );
    }
    m_coreTypes = std::move( types );
    if ( m_coreLoadGrid )
      m_coreLoadGrid->setTopology( m_coreTypes );
  } );
}

void MonitorTab::sampleAndUpdateCoreLoad()
{
  QFile statFile( QStringLiteral( "/proc/stat" ) );
  if ( !statFile.open( QIODevice::ReadOnly | QIODevice::Text ) )
    return;

  struct ParsedCore
  {
    int     idx = -1;
    quint64 busy = 0;
    quint64 total = 0;
  };
  QVector< ParsedCore > parsed;
  const QByteArray raw = statFile.readAll();
  for ( const QByteArray &line : raw.split( '\n' ) )
  {
    if ( !line.startsWith( "cpu" ) )
      continue;
    if ( line.size() < 4 || line[ 3 ] < '0' || line[ 3 ] > '9' )
      continue; // skip aggregate "cpu " line
    const QList< QByteArray > parts = line.simplified().split( ' ' );
    if ( parts.size() < 5 )
      continue;
    bool okIdx = false;
    const int idx = parts[ 0 ].mid( 3 ).toInt( &okIdx );
    if ( !okIdx || idx < 0 )
      continue;

    quint64 total = 0;
    for ( int col = 1; col < parts.size(); ++col )
      total += parts[ col ].toULongLong();
    const quint64 idle = parts[ 4 ].toULongLong();
    const quint64 iowait = ( parts.size() > 5 ) ? parts[ 5 ].toULongLong() : 0ULL;
    const quint64 idleAll = idle + iowait;
    const quint64 busy = ( total > idleAll ) ? ( total - idleAll ) : 0ULL;
    parsed.push_back( { idx, busy, total } );
  }
  if ( parsed.isEmpty() )
    return;

  int maxIdx = 0;
  for ( const auto &c : parsed )
    maxIdx = std::max( maxIdx, c.idx );
  const int coreCount = maxIdx + 1;
  if ( m_prevCoreSamples.size() != static_cast< size_t >( coreCount ) )
    m_prevCoreSamples.assign( static_cast< size_t >( coreCount ), CoreCpuSample{} );

  QVector< double > loads( coreCount, -1.0 );
  double sumPct = 0.0;
  int activeSamples = 0;
  int peakCore = -1;
  double peakPct = -1.0;

  for ( const auto &c : parsed )
  {
    const size_t i = static_cast< size_t >( c.idx );
    auto &prev = m_prevCoreSamples[ i ];
    if ( prev.valid && c.total > prev.total && c.busy >= prev.busy )
    {
      const quint64 dTotal = c.total - prev.total;
      const quint64 dBusy = c.busy - prev.busy;
      const double pct = std::clamp( 100.0 * static_cast< double >( dBusy ) / static_cast< double >( dTotal ), 0.0, 100.0 );
      loads[ c.idx ] = pct;
      sumPct += pct;
      ++activeSamples;
      if ( pct > peakPct )
      {
        peakPct = pct;
        peakCore = c.idx;
      }
    }
    prev.busy = c.busy;
    prev.total = c.total;
    prev.valid = true;
  }

  if ( m_coreLoadGrid )
    m_coreLoadGrid->setCoreLoads( loads );

  if ( m_coreLoadSummary )
  {
    if ( activeSamples > 0 && peakCore >= 0 )
    {
      const int totalPct = qRound( sumPct / activeSamples );
      const QString peakId = QStringLiteral( "C%1" ).arg( peakCore, 2, 10, QLatin1Char( '0' ) );
      const QString peakType = m_coreTypes.value( peakCore );
      const QString peakTag = ( peakType == QLatin1String( "P" ) || peakType == QLatin1String( "E" ) )
        ? QStringLiteral( "%1 (%2)" ).arg( peakId, peakType )
        : peakId;
      m_coreLoadSummary->setText(
        QStringLiteral( "Total: %1% · Peak: %2 %3%" ).arg( totalPct ).arg( peakTag ).arg( qRound( peakPct ) ) );
    }
    else
    {
      m_coreLoadSummary->setText( QStringLiteral( "%1 cores · Sampling…" ).arg( coreCount ) );
    }
  }
}

// Incremental data fetch

void MonitorTab::fetchData()
{
  if (!m_client || m_paused || !m_monitoringActive || m_fetchPending) return;
  sampleAndUpdateCoreLoad();
  m_fetchPending = true;
  const auto generation = m_fetchGeneration;
  // Query with a small overlap window so late samples from asynchronous daemon producers
  // (e.g. fan ticks vs CPU/GPU telemetry polls) are never skipped by a single global cursor (`TA-19`).
  static constexpr qint64 kFetchOverlapMs = 2000;
  const qint64 fetchSince = ( m_lastTimestamp > kFetchOverlapMs )
    ? ( m_lastTimestamp - kFetchOverlapMs )
    : 0;
  readUccdAsync(this, "GetMonitorDataSince", {QVariant::fromValue<qlonglong>(fetchSince)},
    [this, generation](std::optional<QVariant> result) {
      m_fetchPending = false;
      if (!m_monitoringActive || m_paused || generation != m_fetchGeneration ||
          !result || result->toByteArray().isEmpty()) return;
      // Suspend painting on ALL chart views during the batch update
      m_tempChartView->setUpdatesEnabled( false );
      m_dutyChartView->setUpdatesEnabled( false );
      m_powerChartView->setUpdatesEnabled( false );
      m_freqChartView->setUpdatesEnabled( false );
      m_voltChartView->setUpdatesEnabled( false );
      m_unifiedChartView->setUpdatesEnabled( false );

      applyBinaryData(result->toByteArray());
      trimSeries();
      commitSeries();
      updateAxes();
      updateStickyMarkPositions();

      // Resume painting - triggers a single composite repaint
      m_tempChartView->setUpdatesEnabled( true );
      m_dutyChartView->setUpdatesEnabled( true );
      m_powerChartView->setUpdatesEnabled( true );
      m_freqChartView->setUpdatesEnabled( true );
      m_voltChartView->setUpdatesEnabled( true );
      m_unifiedChartView->setUpdatesEnabled( true );

      // Refresh floating crosshair labels so they don't lag behind the scrolling data
      if ( m_cursorInPlot )
        updateCrosshair( m_lastCrosshairPos, m_annotationsVisible );
    });
}

// Pause / resume via spacebar

void MonitorTab::keyPressEvent( QKeyEvent *event )
{
  if ( event->key() == Qt::Key_Space )
  {
    m_paused = !m_paused;
    syncFrameTimer();
    if ( !m_paused && m_zoomed )
      resetZoom();
    updateMonitorStatus();
    event->accept();
    return;
  }

  QWidget::keyPressEvent( event );
}

void MonitorTab::applyBinaryData( const QByteArray &data )
{
  // Wire layout (native endian - same-host IPC):
  //   per non-empty metric: uint8_t metricId, uint32_t count,
  //                         count * { int64_t timestampMs, double value } (16 bytes each)
  //
  // Points are appended to in-memory buffers only. The actual QLineSeries
  // objects are updated in commitSeries() via a single replace() call,
  // which emits exactly one signal per series instead of one per append +
  // one per trim.

  static constexpr size_t kPointSize = sizeof( int64_t ) + sizeof( double );  // 16

  const auto *p   = reinterpret_cast< const uint8_t * >( data.constData() );
  const auto *end = p + data.size();
  qint64 maxTs = m_lastTimestamp;

  while ( p < end )
  {
    if ( static_cast<size_t>(end - p) < 1 + sizeof(uint32_t) )
      break;

    const uint8_t metricId = *p++;

    uint32_t count = 0;
    std::memcpy( &count, p, sizeof( count ) );
    p += sizeof( count );

    if ( count > static_cast<size_t>(end - p) / kPointSize )
      break;

    const bool valid = ( metricId < METRIC_COUNT )
                       && ( m_seriesMap.count( kMetrics[ metricId ].key ) > 0 );

    for ( uint32_t j = 0; j < count; ++j )
    {
      int64_t ts  = 0;
      double  val = 0.0;
      std::memcpy( &ts,  p,             sizeof( ts ) );
      std::memcpy( &val, p + sizeof(ts), sizeof( val ) );
      p += kPointSize;

      if ( valid )
      {
        auto &info = m_seriesMap[ kMetrics[ metricId ].key ];
        const qreal tsReal = static_cast< qreal >( ts );
        if ( info.buffer.isEmpty() || ts > info.lastTimestamp )
        {
          info.buffer.append( QPointF( tsReal, val ) );
          if ( ts > info.lastTimestamp )
            info.lastTimestamp = ts;
        }
        else
        {
          // Late sample from a slower producer inside the overlap window: insert in
          // sorted order if not already present so zero points are lost or duplicated (`TA-19`).
          qsizetype pos = info.buffer.size();
          bool duplicate = false;
          while ( pos > 0 )
          {
            const qint64 existingTs = qRound64( info.buffer[ pos - 1 ].x() );
            if ( existingTs == ts )
            {
              duplicate = true;
              break;
            }
            if ( existingTs < ts )
              break;
            --pos;
          }
          if ( !duplicate )
          {
            info.buffer.insert( pos, QPointF( tsReal, val ) );
          }
        }
        if ( ts > maxTs )
          maxTs = ts;
      }
    }
  }

  // Advance high-water timestamp (without +1 skipping concurrent producers).
  if ( maxTs > m_lastTimestamp )
    m_lastTimestamp = maxTs;
}

void MonitorTab::trimSeries()
{
  const qint64 now = QDateTime::currentMSecsSinceEpoch();
  const qreal cutoff = static_cast< qreal >( now - m_fetchTimer.interval() - static_cast< qint64 >( m_windowSeconds ) * 1000 );

  // Trim leading stale points from in-memory buffers only.
  // The QLineSeries objects are updated in commitSeries().
  for ( auto &[key, info] : m_seriesMap )
  {
    auto &buf = info.buffer;
    int stale = 0;
    while ( stale < buf.size() && buf[ stale ].x() < cutoff )
      ++stale;
    // Keep one predecessor so the line reaches the left viewport edge.
    if ( stale > 1 )
      buf.remove( 0, stale - 1 );
  }
}

void MonitorTab::commitSeries()
{
  // Push in-memory buffers into QLineSeries via a single replace() call.
  // This emits exactly ONE pointsReplaced signal per series, instead of
  // separate pointsAdded + pointsRemoved signals from append() + removePoints().

  for ( int i = 0; i < METRIC_COUNT; ++i )
  {
    auto &info = m_seriesMap[ kMetrics[ i ].key ];
    info.series->replace( info.buffer );

    // Update unified shadow series if active - build normalised list on the fly
    QVariant uv = info.series->property( "_uniSeries" );
    if ( uv.isValid() )
    {
      auto *uni = qobject_cast< QLineSeries * >( uv.value< QObject * >() );
      if ( uni )
      {
        const double scale = metricToNormalisedScale( kMetrics[ i ].group );
        QList< QPointF > scaled;
        scaled.reserve( info.buffer.size() );
        for ( const auto &pt : info.buffer )
          scaled.append( QPointF( pt.x(), pt.y() * scale ) );
        uni->replace( scaled );
      }
    }
  }
}

void MonitorTab::updateAxes()
{
  if (!m_paused || !m_presentationTimestamp)
    m_presentationTimestamp = QDateTime::currentMSecsSinceEpoch() - m_fetchTimer.interval();
  const QDateTime now = QDateTime::fromMSecsSinceEpoch(m_presentationTimestamp);
  const QDateTime start = now.addSecs( -m_windowSeconds );

  // Only update axes for the currently visible chart view - the invisible
  // ones will be updated when they become visible again.
  const bool perGroup = ( m_chartStack->currentIndex() == 0 );

  if ( perGroup )
  {
    auto setRange = [&]( QDateTimeAxis *axis, QChartView *view ) {
      if ( axis && view->isVisible() ) axis->setRange( start, now );
    };
    setRange( m_tempXAxis, m_tempChartView );
    setRange( m_dutyXAxis, m_dutyChartView );
    setRange( m_powerXAxis, m_powerChartView );
    setRange( m_freqXAxis, m_freqChartView );
    setRange( m_voltXAxis, m_voltChartView );
  }
  else
  {
    if ( m_unifiedXAxis && !m_zoomed )
      m_unifiedXAxis->setRange( start, now );
  }
}

// Group chart visibility - hide empty groups, stretch remaining

void MonitorTab::updateGroupChartVisibility()
{
  // Map each MetricGroup to its per-group chart view
  const std::pair< MetricGroup, QChartView * > groupViews[] = {
    { MetricGroup::Temp,  m_tempChartView  },
    { MetricGroup::Duty,  m_dutyChartView  },
    { MetricGroup::Power, m_powerChartView },
    { MetricGroup::Freq,  m_freqChartView  },
    { MetricGroup::Volt,  m_voltChartView  },
  };

  for ( const auto &[group, view] : groupViews )
  {
    bool anyEnabled = false;
    for ( int i = 0; i < METRIC_COUNT; ++i )
    {
      if ( kMetrics[ i ].group == group )
      {
        auto it = m_seriesMap.find( kMetrics[ i ].key );
        if ( it != m_seriesMap.end() && it->second.toggle->isChecked() )
        {
          anyEnabled = true;
          break;
        }
      }
    }
    view->parentWidget()->setVisible( anyEnabled );
  }
  if ( m_coreLoadCard && m_coreLoadCheckBox )
    m_coreLoadCard->setVisible( m_coreLoadCheckBox->isChecked() );
}

void MonitorTab::saveCheckboxStates()
{
  if (readOnlyPreview) return;

  QSettings settings( QDir::homePath() + "/.config/uccrc", QSettings::IniFormat );
  settings.beginGroup( "MonitorTab" );

  // Save individual metric visibility states
  for ( const auto &[key, info] : m_seriesMap )
  {
    settings.setValue( QString::fromStdString( key ), info.toggle->isChecked() );
  }
  if ( m_coreLoadCheckBox )
    settings.setValue( "cpuCoreLoad", m_coreLoadCheckBox->isChecked() );

  // Save unified mode checkbox state
  settings.setValue( "UnifiedMode", m_unifiedCheckBox->isChecked() );

  settings.endGroup();
  settings.sync();
}

void MonitorTab::loadCheckboxStates()
{
  QSettings settings( QDir::homePath() + "/.config/uccrc", QSettings::IniFormat );
  settings.beginGroup( "MonitorTab" );

  // Load individual metric visibility states (default to checked)
  for ( auto &[key, info] : m_seriesMap )
  {
    const bool checked = settings.value( QString::fromStdString( key ), true ).toBool();
    info.toggle->setChecked( checked );
  }
  if ( m_coreLoadCheckBox )
    m_coreLoadCheckBox->setChecked( settings.value( "cpuCoreLoad", true ).toBool() );

  // Load time window in seconds (default 5 min = 300s)
  m_windowSeconds = std::clamp( settings.value( "TimeWindowSeconds", 300 ).toInt(), 60, 1800 );

  // Load unified mode checkbox state (default to false)
  const bool unifiedMode = settings.value( "UnifiedMode", false ).toBool();
  m_unifiedCheckBox->setChecked( unifiedMode );

  settings.endGroup();
}

} // namespace ucc
