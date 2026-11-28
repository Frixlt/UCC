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

#include "MainWindow.hpp"
#include "PowerDevicesTab.hpp"
#include "LightbarTab.hpp"
#include "SessionProcessesWidget.hpp"
#include "DesktopPreferences.hpp"
#include <QVBoxLayout>
#include "FluentTheme.hpp"
#include "NotificationCenter.hpp"
#include "AboutPage.hpp"
#include "AppLogging.hpp"
#include "FluentSidebar.hpp"
#include "FluentEntrance.hpp"
#include "PreviewMode.hpp"
#include "BatterySettingsWidget.hpp"
#include <QTabBar>
#include <QDialog>
#include <QFormLayout>
#include <QSignalBlocker>
#include <QTimer>
#include <QPainter>
#include <QStyleOptionComboBox>
#include "ProfileManager.hpp"
#include "SystemMonitor.hpp"
#include "MonitorTab.hpp"

#include "FanControlTab.hpp"
#include "FanCurveEditorWidget.hpp"
#include "PumpCurveEditorWidget.hpp"
#include "../libucc-dbus/UccdClient.hpp"


#include <QtWidgets/QTableWidget>
#include <QtWidgets/QHeaderView>
#include <QUuid>
#include <QJsonDocument>
#include <QJsonObject>
#include <QJsonArray>
#include <QDBusConnectionInterface>
#include <QDBusMessage>
#include <QDBusPendingCall>
#include <QDBusPendingCallWatcher>
#include <QDBusReply>
#include <QEventLoop>
#include <algorithm>
#include <set>
#include <optional>

namespace {
QLabel *addSidebarStatusRow(QVBoxLayout *layout, QLabel *text, const QString &marker = {},
                           const QString &status = "muted") {
  auto *row = new QWidget;
  auto *line = new QHBoxLayout(row);
  line->setContentsMargins(0,0,0,0);line->setSpacing(8);
  auto *indicator = new QLabel(marker);
  indicator->setObjectName("sidebarStatusIndicator");indicator->setFixedWidth(12);
  indicator->setAlignment(Qt::AlignLeft | Qt::AlignTop);indicator->setProperty("status",status);
  text->setObjectName("sidebarStatusText");text->setWordWrap(true);
  line->addWidget(indicator);line->addWidget(text,1);layout->addWidget(row);
  return indicator;
}
}

// Helper widget for rotated y-axis label
class RotatedLabel : public QLabel
{
public:
  explicit RotatedLabel( const QString &text, QWidget *parent = nullptr )
    : QLabel( text, parent )
  {
  }

protected:
  void paintEvent( QPaintEvent *event ) override
  {
    ( void )event;
    QPainter p( this );
    p.setRenderHint( QPainter::Antialiasing );
    p.translate( width() / 2.0, height() / 2.0 );
    p.rotate( -90 );
    p.translate( -height() / 2.0, -width() / 2.0 );
    QRect r( 0, 0, height(), width() );
    p.setPen( QColor( "#bdbdbd" ) );
    QFont f = font();
    f.setPointSize( 11 );
    p.setFont( f );
    p.drawText( r, Qt::AlignCenter, "% Duty" );
  }

  QSize minimumSizeHint() const override
  {
    return QSize( 24, 80 );
  }

  QSize sizeHint() const override
  {
    return QSize( 24, 120 );
  }
};
#include <QWidget>
#include <QVBoxLayout>
#include <QHBoxLayout>
#include <QGridLayout>
#include <QGroupBox>
#include <QPushButton>
#include <QTabWidget>
#include <QSlider>
#include <QSpinBox>
#include <QCheckBox>
#include <QTextEdit>
#include <QScrollArea>
#include <QListWidget>
#include <QLineEdit>
#include <QMessageBox>
#include <QMenu>
#include <QAction>
#include <QJsonDocument>
#include <QJsonObject>
#include <QDebug>
#include <QShowEvent>
#include <QHideEvent>

namespace ucc
{

MainWindow::MainWindow( QWidget *parent )
  : QMainWindow( parent )
  , m_profileManager( std::make_unique< ProfileManager >( this ) )
  , m_systemMonitor( std::make_unique< SystemMonitor >( this ) )
{
  m_UccdClient = std::make_unique< UccdClient >( this );

  // Defer D-Bus capability queries asynchronously so MainWindow construction never blocks the UI thread (`N12`).
  m_waterCoolerSupported = ( m_UccdClient && m_UccdClient->isConnected() );

  setWindowTitle( "ucc-rusted" );
  resize(1280,900);
  if (readOnlyPreview) setWindowTitle("ucc-rusted · Read-only preview");

  setAttribute(Qt::WA_TranslucentBackground);
  setupUI();

  // Connect signals after UI elements are created but before loading data
  connectSignals();

  auto *statusLayout=findChild<QVBoxLayout *>("sidebarStatusLayout");
  // Status information lives in the navigation footer.
  // Water cooler status indicator (left of the connection indicator)
  if ( m_waterCoolerSupported )
  {
    m_waterCoolerStatusBarLabel = new QLabel( this );
    m_waterCoolerStatusBarLabel->setTextFormat( Qt::PlainText );
    m_waterCoolerStatusBarLabel->setWordWrap(true);
    addSidebarStatusRow(statusLayout,m_waterCoolerStatusBarLabel,"●","accent");

    connect( m_dashboardTab, &DashboardTab::waterCoolerStatusChanged,
             m_waterCoolerStatusBarLabel, &QLabel::setText );
    // Populate the label with the current status immediately
    m_dashboardTab->refreshWaterCoolerStatus();
  }

  m_connectionLabel = new QLabel( this );
  m_connectionLabel->setTextFormat( Qt::PlainText );
  addSidebarStatusRow(statusLayout,m_connectionLabel);
  statusBar()->hide();
  // Set initial connection state
  onUccdConnectionChanged( m_UccdClient->isConnected() );
  statusBar()->showMessage( "Ready" );

  QDBusConnection::systemBus().connect(
    QStringLiteral( "org.freedesktop.login1" ),
    QStringLiteral( "/org/freedesktop/login1" ),
    QStringLiteral( "org.freedesktop.login1.Manager" ),
    QStringLiteral( "PrepareForSleep" ),
    this,
    SLOT( onPrepareForSleep(bool) ) );

  // Load initial data - refresh() emits signals that populate the UI.
  // Block the profile combo to avoid cascading loadProfileDetails calls.
  m_profileCombo->blockSignals( true );
  m_profileManager->refresh();
  m_profileCombo->blockSignals( false );

  // Now populate the combo and load the active profile exactly once.
  onAllProfilesChanged();

  // Initialize current fan profile ID to first available fan profile (if any)
  m_currentFanProfile = ( m_fanControlTab && m_fanControlTab->fanProfileCombo() && m_fanControlTab->fanProfileCombo()->count() > 0 )
    ? m_fanControlTab->fanProfileCombo()->currentData().toString() : QString();

  // Startup complete - allow hardware interaction from now on
  m_initializing = false;

  // Start monitoring since dashboard is the first tab
  updateMonitoringActivity();
}

MainWindow::~MainWindow()
{
  for(auto *child:findChildren<QObject*>()) QObject::disconnect(child,nullptr,this,nullptr);
}

void MainWindow::showEvent(QShowEvent *event)
{
  QMainWindow::showEvent(event);
  updateMonitoringActivity();
  if(m_entrance) m_entrance->play();
}

void MainWindow::hideEvent(QHideEvent *event)
{
  QMainWindow::hideEvent(event);
  updateMonitoringActivity();
}

void MainWindow::closeEvent(QCloseEvent *event)
{
  m_closing = true;
  if ( m_pendingApplyWatcher )
  {
    m_pendingApplyWatcher->disconnect( this );
    m_pendingApplyWatcher->deleteLater();
    m_pendingApplyWatcher.clear();
  }
  QMainWindow::closeEvent( event );
}

void MainWindow::changeEvent(QEvent *event)
{
  QMainWindow::changeEvent(event);
  if (event->type() == QEvent::WindowStateChange) updateMonitoringActivity();
}

void MainWindow::updateMonitoringActivity()
{
  if (!m_tabs || !m_systemMonitor) return;
  const bool visible = isVisible() && !isMinimized();
  const int index = m_tabs->currentIndex();
  const int fanIndex = m_fanControlTab ? m_tabs->indexOf(m_fanControlTab) : -1;
  m_systemMonitor->setMonitoringActive(visible && (index == 0 || index == fanIndex));
  if (m_monitorTab)
    m_monitorTab->setMonitoringActive(visible && index == m_tabs->indexOf(m_monitorTab));
  if(auto *processes=findChild<SessionProcessesWidget*>()) {
    QWidget *currentTab = m_tabs->widget(index);
    processes->monitor()->setActive(visible && currentTab && currentTab->isAncestorOf(processes));
  }
}

void MainWindow::setupUI()
{
  // Create tab widget
  m_tabs = new QTabWidget( this );
  m_tabs->setObjectName("pages");
  m_tabs->tabBar()->hide();
  auto *shell=new QWidget(this);
  auto *shellLayout=new QHBoxLayout(shell);
  shellLayout->setContentsMargins(0,0,0,0); shellLayout->setSpacing(0);
  auto *sidebar=new FluentSidebar(shell);
  sidebar->setFixedWidth(250);
  auto *sideLayout=new QVBoxLayout(sidebar);sideLayout->setContentsMargins(12,28,12,20);
  auto *brand=new QLabel("ucc-rusted",sidebar);brand->setObjectName("brand");
  sideLayout->addWidget(brand);sideLayout->addSpacing(12);
  auto *device=new QLabel("ucc-rusted",sidebar);device->setWordWrap(true);device->setObjectName("muted");
  sideLayout->addWidget(device);sideLayout->addSpacing(20);
  auto *navigation=new QListWidget(sidebar);navigation->setObjectName("navigation");
  navigation->setIconSize(QSize(24,24));
  const QStringList labels={"Overview","Profiles","Cooler Settings","Watercool Settings","Monitor","Lighting","Power & Devices","Battery & Processes","Experimental"};
  for(const auto &label:labels) new QListWidgetItem(FluentTheme::icon(label),label,navigation);
  sideLayout->addWidget(navigation,1);
  m_aboutNavButton=new QPushButton(FluentTheme::icon("About"),"About",sidebar);
  m_aboutNavButton->setObjectName("aboutNavigation");
  m_aboutNavButton->setCheckable(true);
  m_aboutNavButton->setIconSize(QSize(24,24));
  sideLayout->addSpacing(4);
  sideLayout->addWidget(m_aboutNavButton);
  auto *footer=new QWidget(sidebar);footer->setObjectName("sidebarFooter");
  auto *statusLayout=new QVBoxLayout(footer);statusLayout->setObjectName("sidebarStatusLayout");statusLayout->setContentsMargins(12,16,12,0);statusLayout->setSpacing(8);
  if(readOnlyPreview) {
    auto *badge=new QLabel("Read-only preview",footer);badge->setObjectName("previewBadge");
    badge->setToolTip("Hardware and settings commands are blocked in this build.");statusLayout->addWidget(badge);
  }
  m_sidebarConnectionLabel=new QLabel(footer);
  m_sidebarConnectionIndicator=addSidebarStatusRow(statusLayout,m_sidebarConnectionLabel,"●");
  sideLayout->addWidget(footer);
  m_resumeGraceTimer = new QTimer( this );
  m_resumeGraceTimer->setSingleShot( true );
  m_resumeGraceTimer->setInterval( 15000 );
  connect( m_resumeGraceTimer, &QTimer::timeout, this, [this] {
    if ( !m_resumingFromSleep )
      return;
    m_resumingFromSleep = false;
    updateSidebarConnectionStatus( m_UccdClient->isConnected() );
    if ( m_connectionLabel && !m_UccdClient->isConnected() )
      m_connectionLabel->setText( QStringLiteral( "Profile: —" ) );
  } );
  updateSidebarConnectionStatus(m_UccdClient->isConnected());
  connect(m_UccdClient.get(),&UccdClient::connectionStatusChanged,this,&MainWindow::updateSidebarConnectionStatus);
  shellLayout->addWidget(sidebar);
  auto *content=new QWidget(shell);content->setObjectName("contentPane");content->setAttribute(Qt::WA_StyledBackground);auto *contentLayout=new QVBoxLayout(content);
  contentLayout->setContentsMargins(24,24,24,16);contentLayout->setSpacing(16);
  auto *heading=new QLabel("Overview",content);heading->setObjectName("pageTitle");
  auto *headerRow=new QHBoxLayout;headerRow->addWidget(heading,1);
  auto *powerMode = new QComboBox(content);
  powerMode->setObjectName("powerModeSelector");
  powerMode->setAccessibleName("Power mode");
  powerMode->addItem("Auto", "auto");
  powerMode->addItem("AC", "ac");
  powerMode->addItem("Type-C", "type_c");
  powerMode->addItem("Battery", "bat");
  powerMode->setEnabled(!readOnlyPreview && m_UccdClient->isConnected());
  auto showPowerPolicy = [powerMode](const QString &json) {
    const auto policy = QJsonDocument::fromJson(json.toUtf8()).object();
    if (policy.isEmpty()) return;
    const QSignalBlocker blocker(powerMode);
    powerMode->setCurrentIndex(qMax(0,powerMode->findData(policy["manual_mode"].toString("auto"))));
    powerMode->setToolTip(QString("Source: %1 · Effective mode: %2 · Profile state: %3")
      .arg(policy["real_power_source"].toString(),policy["current_mode"].toString(),policy["profile_state"].toString()));
  };
  auto refreshPowerPolicy = [this,showPowerPolicy] {
    if (auto policy = m_UccdClient->getPowerPolicyJSON()) showPowerPolicy(QString::fromStdString(*policy));
  };
  connect(m_UccdClient.get(), &UccdClient::powerPolicyChanged, this, showPowerPolicy);
  connect(m_UccdClient.get(), &UccdClient::connectionStatusChanged, this,
    [powerMode,refreshPowerPolicy](bool connected) {
      powerMode->setEnabled(connected && !readOnlyPreview);
      if (connected) refreshPowerPolicy();
    });
  connect(powerMode, QOverload<int>::of(&QComboBox::activated), this,
    [this,powerMode,refreshPowerPolicy](int index) {
      if (!m_UccdClient->setManualPowerMode(powerMode->itemData(index).toString().toStdString())) {
        statusBar()->showMessage("Could not change power mode");
        refreshPowerPolicy();
      }
    });
  refreshPowerPolicy();
  headerRow->addWidget(powerMode);
  auto *themeToggle=new QPushButton(content);themeToggle->setObjectName("themeToggle");themeToggle->setFixedSize(36,34);themeToggle->setIconSize(QSize(22,22));themeToggle->setStyleSheet("padding: 5px;");
  themeToggle->setAccessibleName("Interface theme");
  auto *notificationBell=new QPushButton(content);notificationBell->setObjectName("notificationBell");
  notificationBell->setFixedSize(36,34);notificationBell->setIconSize(QSize(22,22));notificationBell->setStyleSheet("padding: 5px;");
  auto *notifications=new NotificationCenter(notificationBell,this);
  headerRow->addWidget(notificationBell,0,Qt::AlignTop);
  connect(statusBar(),&QStatusBar::messageChanged,notifications,[this,notifications,navigation](const QString &message){
    const int row=navigation->currentRow();
    const QString section=(m_aboutNavButton && m_aboutNavButton->isChecked())
      ? QStringLiteral("About")
      : (row>=0 && navigation->item(row) ? navigation->item(row)->text() : QStringLiteral("ucc-rusted"));
    notifications->addEvent(section,message);
    if(AppLogging::isEnabled() && !message.trimmed().isEmpty() && message!="Ready")
      qInfo().noquote()<<"[UI]"<<section<<message;
  });
  headerRow->addWidget(themeToggle,0,Qt::AlignTop);contentLayout->addLayout(headerRow);
  auto refreshTheme=[this,themeToggle,navigation,sidebar]{
    themeToggle->setIcon(FluentTheme::icon(FluentTheme::modeLabel()));
    themeToggle->setAccessibleName("Theme: "+FluentTheme::modeLabel());
    themeToggle->setToolTip("Theme: "+FluentTheme::modeLabel()+" · Light → Dark → Auto (system theme)");
    for(int i=0;i<navigation->count();++i) navigation->item(i)->setIcon(FluentTheme::icon(navigation->item(i)->text()));
    if(m_aboutNavButton) m_aboutNavButton->setIcon(FluentTheme::icon("About"));
    sidebar->update();
  };
  connect(themeToggle,&QPushButton::clicked,this,[]{
    using FluentTheme::Mode;const auto next=FluentTheme::mode()==Mode::Light ? Mode::Dark : FluentTheme::mode()==Mode::Dark ? Mode::Auto : Mode::Light;
    FluentTheme::setMode(next);
  });
  connect(FluentTheme::events(),&FluentTheme::ThemeEvents::changed,this,refreshTheme);refreshTheme();
  auto *subtitle=new QLabel("Your system at a glance",content);subtitle->setObjectName("subtitle");subtitle->setWordWrap(true);contentLayout->addWidget(subtitle);
  contentLayout->addWidget(m_tabs,1);shellLayout->addWidget(content,1);setCentralWidget(shell);
  m_entrance=new FluentEntrance(content);
  auto showNavigationHeading=[heading,subtitle,labels](int row) {
    if(row==9) {
      heading->setText("About");
      subtitle->setText("Build information, project origins and open-source acknowledgements");
      return;
    }
    if(row<0 || row>=labels.size()) return;
    heading->setText(labels[row]);
    const QStringList descriptions={"Your system at a glance","Power, cooling and lighting in one profile","Shape CPU and GPU cooling for your profile","Water cooling, pump and lighting controls","Live system telemetry","Keyboard backlight, lightbar and chassis logo lighting","CPU, modules and device policies for every power source","Battery power and your NVIDIA processes","Preview and experimental settings under development"};
    subtitle->setText(descriptions.value(row));
  };
  connect(navigation,&QListWidget::currentRowChanged,this,[this,showNavigationHeading](int row){
    if(row<0 || row>8) return;
    const int pages[]={0,1,2,2,3,4,5,6,7};
    if(row==3 && !m_waterCoolerSupported) return;
    if(m_aboutNavButton) {
      const QSignalBlocker blockAbout(m_aboutNavButton);
      m_aboutNavButton->setChecked(false);
    }
    if(m_fanControlTab && (row==2 || row==3)) {
      m_fanControlTab->setWaterCoolerMode(row==3);
      m_fanControlTab->findChild<QTabWidget *>("coolingPages")->setCurrentIndex(row-2);
    }
    const bool samePage=m_tabs->currentIndex()==pages[row];
    m_tabs->setCurrentIndex(pages[row]);showNavigationHeading(row);
    if(row==2 || row==3) updateFanCrosshairs();
    if(samePage) m_entrance->play();
  });
  connect(m_aboutNavButton,&QPushButton::clicked,this,[this,navigation,showNavigationHeading]{
    const QSignalBlocker blockNav(navigation);
    navigation->clearSelection();
    navigation->setCurrentRow(-1);
    if(m_aboutNavButton) {
      const QSignalBlocker blockAbout(m_aboutNavButton);
      m_aboutNavButton->setChecked(true);
    }
    const bool samePage=m_tabs->currentIndex()==8;
    m_tabs->setCurrentIndex(8);showNavigationHeading(9);
    if(samePage) m_entrance->play();
  });
  connect(m_tabs,&QTabWidget::currentChanged,navigation,[this,navigation,showNavigationHeading](int index){
    if(index<0 || index>8) return;
    if(index==8) {
      const QSignalBlocker blockNav(navigation);
      navigation->clearSelection();
      navigation->setCurrentRow(-1);
      if(m_aboutNavButton) {
        const QSignalBlocker blockAbout(m_aboutNavButton);
        m_aboutNavButton->setChecked(true);
      }
      showNavigationHeading(9);
      m_entrance->play();
      return;
    }
    if(m_aboutNavButton) {
      const QSignalBlocker blockAbout(m_aboutNavButton);
      m_aboutNavButton->setChecked(false);
    }
    int row=index<3 ? index : index+1;
    if(index==2 && m_fanControlTab) row=2+m_fanControlTab->findChild<QTabWidget *>("coolingPages")->currentIndex();
    const QSignalBlocker blocked(navigation);navigation->setCurrentRow(row);showNavigationHeading(row);
    m_entrance->play();
  });
  // Connect tab changes to control monitoring
  connect( m_tabs, &QTabWidget::currentChanged, this, &MainWindow::onTabChanged );

  // Fetch system hardware info from daemon and pass to DashboardTab
  QString laptopModel, cpuModel, dGpuModel, iGpuModel;
  if ( auto sysInfoJson = m_UccdClient->getSystemInfoJSON() )
  {
    QJsonDocument doc = QJsonDocument::fromJson( QByteArray::fromStdString( *sysInfoJson ) );
    if ( doc.isObject() )
    {
      const QJsonObject obj = doc.object();
      laptopModel = obj.value( "laptopModel" ).toString();
      if (!laptopModel.isEmpty()) device->setText(laptopModel);
      cpuModel    = obj.value( "cpuModel" ).toString();
      dGpuModel   = obj.value( "dGpuModel" ).toString();
      iGpuModel   = obj.value( "iGpuModel" ).toString();
    }
  }

  // Now create DashboardTab (daemon-backed water cooler; no controller pointer)
  m_dashboardTab = new DashboardTab( m_systemMonitor.get(), m_profileManager.get(), m_waterCoolerSupported,
                                     laptopModel, cpuModel, dGpuModel, iGpuModel, this );
  m_tabs->addTab( m_dashboardTab, "Dashboard" );
  setupProfilesPage();

  // Place the Fan Control tab directly after Profiles and rename it
  setupFanControlTab();

  // Add Monitoring graph tab
  m_monitorTab = new MonitorTab( m_UccdClient.get(), this );
  m_tabs->addTab( m_monitorTab, "Monitor" );

  setupLightingPage();
  auto *powerDevices=new PowerDevicesTab(m_UccdClient.get(),this);
  connect(powerDevices,&PowerDevicesTab::message,statusBar(),[this](const QString &text){statusBar()->showMessage(text);});
  m_tabs->addTab(powerDevices,"Power & Devices");
  auto *desktopScroll=new QScrollArea;desktopScroll->setWidgetResizable(true);desktopScroll->setFrameShape(QFrame::NoFrame);
  auto *desktopPage=new QWidget;auto *desktopLayout=new QVBoxLayout(desktopPage);desktopLayout->setContentsMargins(0,0,0,0);
  auto *minimized=new QCheckBox("Start Control Center minimized");minimized->setObjectName("startMinimized");minimized->setChecked(startMinimizedPreference());minimized->setEnabled(!readOnlyPreview);desktopLayout->addWidget(minimized);
  connect(minimized,&QCheckBox::toggled,this,[](bool value){if(!readOnlyPreview) QSettings("UniwillControlCenter","desktop").setValue("startMinimized",value);});
  auto *refreshHints=new QCheckBox("Offer display refresh changes on KDE / Hyprland");refreshHints->setObjectName("refreshRateNotifications");refreshHints->setChecked(readOnlyPreview || QSettings("UniwillControlCenter","desktop").value("refreshRateNotifications",true).toBool());refreshHints->setEnabled(!readOnlyPreview);desktopLayout->addWidget(refreshHints);
  connect(refreshHints,&QCheckBox::toggled,this,[](bool value){if(!readOnlyPreview) QSettings("UniwillControlCenter","desktop").setValue("refreshRateNotifications",value);});
  desktopLayout->addWidget(new BatterySettingsWidget(m_UccdClient.get(),desktopPage));
  auto *processes=new SessionProcessesWidget({},nullptr,desktopPage);processes->setMinimumHeight(260);desktopLayout->addWidget(processes,1);
  connect(processes,&SessionProcessesWidget::message,statusBar(),[this](const QString &text){statusBar()->showMessage(text);});
  desktopScroll->setWidget(desktopPage);
  m_tabs->addTab(desktopScroll,"Battery & Processes");
  m_experimentalRisksAccepted = QSettings("UniwillControlCenter", "desktop")
      .value("experimentalRisksAcceptedPermanently", false).toBool();
  m_experimentalPage = new QWidget;
  m_experimentalPage->setObjectName("experimentalPage");
  auto *experimentalLayout = new QVBoxLayout(m_experimentalPage);
  experimentalLayout->setContentsMargins(0, 0, 0, 0);
  experimentalLayout->setSpacing(12);

  auto *experimentalWarningCard = FluentTheme::createCard();
  experimentalWarningCard->setProperty("role", "experimentalWarningCard");
  auto *experimentalWarningLayout = new QVBoxLayout(experimentalWarningCard);
  experimentalWarningLayout->setContentsMargins(20, 16, 20, 16);
  experimentalWarningLayout->setSpacing(8);
  auto *experimentalTitle = new QLabel("Experimental features");
  experimentalTitle->setObjectName("cardTitle");
  experimentalWarningLayout->addWidget(experimentalTitle);
  auto *experimentalBanner = new QLabel(
      "Options in this tab are experimental, unstable, and may modify firmware/BIOS state (such as Display MUX). "
      "All controls stay locked until you accept the risks.");
  experimentalBanner->setObjectName("experimentalBanner");
  experimentalBanner->setWordWrap(true);
  experimentalWarningLayout->addWidget(experimentalBanner);
  m_experimentalUnlockButton = new QPushButton("I accept the risks — unlock Experimental");
  m_experimentalUnlockButton->setObjectName("experimentalUnlockButton");
  m_experimentalUnlockButton->setProperty("primary", true);
  connect(m_experimentalUnlockButton, &QPushButton::clicked, this, &MainWindow::promptExperimentalRisks);
  experimentalWarningLayout->addWidget(m_experimentalUnlockButton, 0, Qt::AlignLeft);
  experimentalLayout->addWidget(experimentalWarningCard);

  m_experimentalOptionsCard = FluentTheme::createCard();
  m_experimentalOptionsCard->setProperty("role", "experimentalOptionsCard");
  auto *experimentalOptionsLayout = new QVBoxLayout(m_experimentalOptionsCard);
  experimentalOptionsLayout->setContentsMargins(20, 18, 20, 18);
  experimentalOptionsLayout->setSpacing(12);
  auto *muxTitle = new QLabel("Display MUX Switch");
  muxTitle->setObjectName("cardTitle");
  experimentalOptionsLayout->addWidget(muxTitle);
  auto *muxDesc = new QLabel(
      "Switch the hardware display multiplexer BIOS boot mode (ACPI MXDM). "
      "Changes take effect after a full reboot.");
  muxDesc->setObjectName("muted");
  muxDesc->setWordWrap(true);
  experimentalOptionsLayout->addWidget(muxDesc);
  auto *muxForm = new QFormLayout();
  muxForm->setContentsMargins(0, 4, 0, 4);
  muxForm->setSpacing(10);
  m_muxActiveGpuLabel = new QLabel("—");
  m_muxActiveGpuLabel->setObjectName("muxActiveGpuLabel");
  muxForm->addRow("Active display GPU", m_muxActiveGpuLabel);
  m_muxBootModeCombo = new QComboBox();
  m_muxBootModeCombo->setObjectName("muxBootModeCombo");
  m_muxBootModeCombo->addItem("Dynamic (Advanced Optimus)", "dynamic");
  m_muxBootModeCombo->addItem("dGPU Only (Discrete)", "dgpu_only");
  m_muxBootModeCombo->addItem("iGPU Only (Integrated)", "igpu_only");
  m_muxBootModeCombo->addItem("MSHybrid (Optimus)", "mshybrid");
  muxForm->addRow("BIOS boot mode", m_muxBootModeCombo);
  experimentalOptionsLayout->addLayout(muxForm);
  auto *muxButtons = new QHBoxLayout();
  m_muxReloadButton = new QPushButton("Reload");
  m_muxReloadButton->setObjectName("muxReloadButton");
  muxButtons->addWidget(m_muxReloadButton);
  muxButtons->addStretch(1);
  m_muxApplyButton = new QPushButton("Apply MUX mode (requires reboot)");
  m_muxApplyButton->setObjectName("muxApplyButton");
  m_muxApplyButton->setProperty("primary", true);
  muxButtons->addWidget(m_muxApplyButton);
  experimentalOptionsLayout->addLayout(muxButtons);
  m_muxStatusLabel = new QLabel("Waiting for daemon…");
  m_muxStatusLabel->setObjectName("muxStatusLabel");
  m_muxStatusLabel->setWordWrap(true);
  experimentalOptionsLayout->addWidget(m_muxStatusLabel);
  connect(m_muxReloadButton, &QPushButton::clicked, this, &MainWindow::refreshMuxState);
  connect(m_muxApplyButton, &QPushButton::clicked, this, &MainWindow::applyMuxBootMode);
  experimentalLayout->addWidget(m_experimentalOptionsCard);
  experimentalLayout->addStretch(1);
  updateExperimentalLockState();
  m_tabs->addTab(m_experimentalPage, "Experimental");
  m_tabs->addTab(createAboutPage(this),"About");
  if(!m_waterCoolerSupported) {navigation->item(3)->setFlags(navigation->item(3)->flags() & ~Qt::ItemIsEnabled);navigation->item(3)->setToolTip("Water cooling is not supported by this device.");}
  navigation->setCurrentRow(0);
}

void MainWindow::setupLightingPage()
{
  auto *lightingPage = new QWidget( this );
  auto *lightingLayout = new QVBoxLayout( lightingPage );
  lightingLayout->setContentsMargins( 0, 0, 0, 0 );
  lightingLayout->setSpacing( 14 );

  auto *segRow = new QHBoxLayout();
  segRow->setContentsMargins( 0, 0, 0, 0 );
  auto *segBar = new QFrame( lightingPage );
  segBar->setObjectName( "lightingSegmentedBar" );
  auto *segLayout = new QHBoxLayout( segBar );
  segLayout->setContentsMargins( 4, 4, 4, 4 );
  segLayout->setSpacing( 4 );
  segRow->addWidget( segBar );
  segRow->addStretch( 1 );
  lightingLayout->addLayout( segRow );

  m_lightingTabs = new QTabWidget( lightingPage );
  m_lightingTabs->setObjectName( "lightingTabs" );
  m_lightingTabs->setTabsClosable( false );
  m_lightingTabs->setMovable( false );
  m_lightingTabs->tabBar()->hide();

  setupKeyboardBacklightPage( m_lightingTabs );

  auto *lightbar = new LightbarTab( m_UccdClient.get(), m_lightingTabs );
  connect( lightbar, &LightbarTab::message, statusBar(),
           [this]( const QString &text ) { statusBar()->showMessage( text ); } );
  m_lightingTabs->addTab( lightbar, "Lightbar" );

  auto *logoTab = new LogoTab( m_UccdClient.get(), m_lightingTabs );
  connect( logoTab, &LogoTab::message, statusBar(),
           [this]( const QString &text ) { statusBar()->showMessage( text ); } );
  m_lightingTabs->addTab( logoTab, "Logo" );

  struct SegmentDef { QString label; QString iconKey; };
  const QVector<SegmentDef> segmentDefs = {
    { "Keyboard", "Keyboard" },
    { "Lightbar", "Lightbar" },
    { "Logo", "Logo" },
  };
  QVector<QPushButton *> segmentButtons;
  segmentButtons.reserve( segmentDefs.size() );
  for ( int i = 0; i < segmentDefs.size(); ++i )
  {
    auto *btn = new QPushButton( segmentDefs[i].label, segBar );
    btn->setObjectName( "lightingSegmentButton" );
    btn->setCheckable( true );
    btn->setCursor( Qt::PointingHandCursor );
    btn->setIconSize( QSize( 18, 18 ) );
    btn->setMinimumWidth( 116 );
    segLayout->addWidget( btn );
    segmentButtons.append( btn );
    connect( btn, &QPushButton::clicked, this, [this, i]() {
      if ( m_lightingTabs )
        m_lightingTabs->setCurrentIndex( i );
    } );
  }
  auto syncSegments = [segmentButtons, segmentDefs]( int activeIndex ) {
    const auto c = FluentTheme::colors();
    for ( int i = 0; i < segmentButtons.size(); ++i )
    {
      const bool active = ( i == activeIndex );
      const QSignalBlocker blocked( segmentButtons[i] );
      segmentButtons[i]->setChecked( active );
      segmentButtons[i]->setIcon( FluentTheme::icon( segmentDefs[i].iconKey, active ? c.accent : c.secondary ) );
    }
  };
  syncSegments( m_lightingTabs->currentIndex() );
  connect( FluentTheme::events(), &FluentTheme::ThemeEvents::changed, segBar, [this, syncSegments]() {
    if ( m_lightingTabs )
      syncSegments( m_lightingTabs->currentIndex() );
  } );
  connect( m_lightingTabs, &QTabWidget::currentChanged, this, [this, syncSegments]( int subIndex ) {
    syncSegments( subIndex );
    if ( m_tabs && m_tabs->currentIndex() == 4 && subIndex == 0 )
      refreshKeyboardLightingState();
  } );

  lightingLayout->addWidget( m_lightingTabs, 1 );
  m_tabs->addTab( lightingPage, "Lighting" );
}

void MainWindow::setupFanControlTab()
{
  m_fanControlTab = new FanControlTab( m_UccdClient.get(), m_profileManager.get(), m_waterCoolerSupported, this );
  connectFanControlTab();

  m_tabs->addTab( m_fanControlTab, "Profile Fan Control" );

  if ( m_fanControlTab->fanProfileCombo()->count() > 0 )
    onFanProfileChanged( m_fanControlTab->fanProfileCombo()->currentData().toString() );
}

void MainWindow::connectFanControlTab()
{
  connect( m_fanControlTab, &FanControlTab::fanProfileChanged,
           this, &MainWindow::onFanProfileChanged );
  connect( m_fanControlTab, &FanControlTab::cpuPointsChanged,
           this, &MainWindow::onCpuFanPointsChanged );
  connect( m_fanControlTab, &FanControlTab::gpuPointsChanged,
           this, &MainWindow::onGpuFanPointsChanged );
  connect( m_fanControlTab, &FanControlTab::wcFanPointsChanged,
           this, &MainWindow::onWaterCoolerFanPointsChanged );
  connect( m_fanControlTab, &FanControlTab::pumpPointsChanged,
           this, &MainWindow::onPumpPointsChanged );
  connect( m_fanControlTab, &FanControlTab::applyRequested,
           this, &MainWindow::onApplyFanProfilesClicked );
  connect( m_fanControlTab, &FanControlTab::saveRequested,
           this, &MainWindow::onSaveFanProfilesClicked );
  connect( m_fanControlTab, &FanControlTab::copyRequested,
           this, &MainWindow::onCopyFanProfileClicked );

  // Bidirectional water-cooler enable checkbox sync
  // FanControlTab toggle -> D-Bus + sync dashboard checkbox
  connect( m_fanControlTab, &FanControlTab::waterCoolerEnableChanged,
           m_dashboardTab, &DashboardTab::setWaterCoolerEnabled );
  // DashboardTab toggle -> D-Bus call + sync fan tab checkbox
  connect( m_dashboardTab, &DashboardTab::waterCoolerEnableChanged,
           this, [this]( bool enabled ) {
             m_fanControlTab->setWaterCoolerEnabled( enabled );
             m_fanControlTab->sendWaterCoolerEnable( enabled );
           } );
  connect( m_fanControlTab, &FanControlTab::removeRequested,
           this, &MainWindow::onRemoveFanProfileClicked );

  // Sync fan profile rename from fan tab to profile page fan combo
  connect( m_fanControlTab, &FanControlTab::fanProfileRenamed,
           this, [this]( const QString &oldName, const QString &newName ) {
    if ( m_profileFanProfileCombo ) {
      if ( int idx = m_profileFanProfileCombo->findText( oldName ); idx != -1 )
        m_profileFanProfileCombo->setItemText( idx, newName );
    }
  } );
}

void MainWindow::setupProfilesPage()
{
  QWidget *profilesWidget = new QWidget();
  QVBoxLayout *mainLayout = new QVBoxLayout( profilesWidget );
  mainLayout->setContentsMargins( 0, 0, 0, 0 );
  mainLayout->setSpacing(16);

  // Create scroll area for the profile content
  QScrollArea *scrollArea = new QScrollArea();
  scrollArea->setWidgetResizable( true );

  QWidget *scrollWidget = new QWidget();
  QVBoxLayout *scrollLayout = new QVBoxLayout( scrollWidget );
  scrollLayout->setContentsMargins( 20, 20, 20, 20 );
  scrollLayout->setSpacing( 15 );

  // Profile Selection ComboBox (in top layout)
  QHBoxLayout *selectLayout = new QHBoxLayout();

  m_profileCombo = FluentTheme::createSelector();
  m_profileCombo->setStyleSheet("QComboBox::down-arrow { image: none; }");
  m_profileCombo->setEditable( true );
  m_profileCombo->setInsertPolicy( QComboBox::NoInsert );
  // Don't populate here - will be done by onAllProfilesChanged signal
  m_profileCombo->setCurrentIndex( m_profileManager->activeProfileIndex() );
  m_selectedProfileIndex = m_profileManager->activeProfileIndex();
  m_applyButton = new QPushButton( "Apply" );
  m_applyButton->setMaximumWidth( 80 );

  m_saveButton = new QPushButton( "Save" );
  m_saveButton->setObjectName("saveProfile");
  m_saveButton->setProperty("primary",true);
  m_saveButton->setMaximumWidth( 80 );
  m_saveButton->setEnabled( false );

  m_copyProfileButton = new QPushButton( "Copy" );
  m_copyProfileButton->setMinimumWidth(80);

  m_removeProfileButton = new QPushButton( "Remove" );
  m_removeProfileButton->setMinimumWidth(90);

  selectLayout->setSpacing(8);
  auto *profileLabel=new QLabel("Profile");profileLabel->setObjectName("muted");
  selectLayout->addWidget(profileLabel);
  m_profileCombo->setSizeAdjustPolicy(QComboBox::AdjustToMinimumContentsLengthWithIcon);
  m_profileCombo->setMinimumContentsLength(10);
  m_profileCombo->setMinimumWidth(180);
  m_profileCombo->setMaximumWidth(360);
  selectLayout->addWidget(m_profileCombo,1);
  selectLayout->addStretch(1);
  selectLayout->addSpacing(8);
  selectLayout->addWidget( m_applyButton );
  selectLayout->addWidget( m_saveButton );
  selectLayout->addWidget( m_copyProfileButton );
  selectLayout->addWidget( m_removeProfileButton );
  mainLayout->addLayout( selectLayout );

  // Now use grid layout for the details
  scrollLayout->setContentsMargins(0,0,0,0);
  scrollLayout->setSpacing(16);
  scrollLayout->setSizeConstraint(QLayout::SetMinAndMaxSize);
  QGridLayout *detailsLayout = new QGridLayout();
  detailsLayout->setSpacing( 12 );
  detailsLayout->setColumnStretch( 0, 0 );  // Labels column - minimal width
  detailsLayout->setColumnStretch( 1, 1 );  // Controls column - expand

  int row = 0;

  // === DESCRIPTION ===
  QLabel *descLabel = new QLabel( "Description" );
  descLabel->setStyleSheet( "font-weight: bold;" );
  m_descriptionEdit = new QTextEdit();
  m_descriptionEdit->setPlainText( "Edit profile to change behaviour" );
  m_descriptionEdit->setMaximumHeight( 60 );
  detailsLayout->addWidget( descLabel, row, 0, Qt::AlignTop );
  detailsLayout->addWidget( m_descriptionEdit, row, 1 );
  row++;

  // === ACTIVATE PROFILE AUTOMATICALLY ON ===
  QLabel *autoActivateLabel = new QLabel( "Activate profile automatically on" );
  autoActivateLabel->setStyleSheet( "font-weight: bold;" );
  QHBoxLayout *buttonLayout = new QHBoxLayout();
  m_mainsButton = new QPushButton( "Mains" );
  m_mainsButton->setCheckable( true );
  m_typeCButton = new QPushButton("Type-C");
  m_typeCButton->setCheckable(true);
  m_typeCButton->setObjectName("typeCProfileAssignment");
  m_batteryButton = new QPushButton( "Battery" );
  m_batteryButton->setCheckable( true );
  m_waterCoolerButton = new QPushButton( "Water Cooler" );
  m_waterCoolerButton->setCheckable( true );
  m_mainsButton->setMaximumWidth( 100 );
  m_batteryButton->setMaximumWidth( 100 );
  m_waterCoolerButton->setMinimumWidth(120);
  buttonLayout->addWidget( m_mainsButton );
  buttonLayout->addWidget(m_typeCButton);
  buttonLayout->addWidget( m_batteryButton );
  buttonLayout->addWidget( m_waterCoolerButton );
  buttonLayout->addStretch();
  detailsLayout->addWidget( autoActivateLabel, row, 0, Qt::AlignTop );
  detailsLayout->addLayout( buttonLayout, row, 1 );
  row++;

  // Hide water cooler profile activation button if water cooler not supported
  if ( !m_waterCoolerSupported )
  {
    m_waterCoolerButton->setVisible( false );
  }

  // Add spacer/separator
  detailsLayout->addItem( new QSpacerItem( 0, 15 ), row, 0, 1, 2 );
  row++;

  // Add spacer before Display section
  detailsLayout->addItem( new QSpacerItem( 0, 10 ), row, 0, 1, 2 );
  row++;

  // === DISPLAY SECTION ===
  QLabel *displayHeader = new QLabel( "Display and Keyboard" );
  displayHeader->setStyleSheet( "font-weight: bold; font-size: 14px;" );
  detailsLayout->addWidget( displayHeader, row, 0, 1, 2 );
  row++;

  QLabel *keyboardProfileLabel = new QLabel( "Keyboard profile" );
  m_profileKeyboardProfileCombo = new QComboBox();
  m_profileKeyboardProfileCombo->setSizeAdjustPolicy( QComboBox::AdjustToMinimumContentsLengthWithIcon );
  m_profileKeyboardProfileCombo->setMinimumContentsLength( 10 );

  for ( const auto &v : m_profileManager->customKeyboardProfilesData() )
  {
    QJsonObject o = v.toObject();
    m_profileKeyboardProfileCombo->addItem( o["name"].toString(), o["id"].toString() );
  }

  detailsLayout->addWidget( keyboardProfileLabel, row, 0 );
  detailsLayout->addWidget( m_profileKeyboardProfileCombo, row, 1 );
  row++;

  QLabel *backlightLabel = new QLabel( "Backlight brightness" );
  QHBoxLayout *backlightLayout = new QHBoxLayout();
  m_brightnessSlider = new QSlider( Qt::Horizontal );
  m_brightnessSlider->setMinimum( 0 );
  m_brightnessSlider->setMaximum( 100 );
  m_brightnessSlider->setValue( 100 );
  m_brightnessValueLabel = new QLabel( "100%" );
  m_brightnessValueLabel->setMinimumWidth( 40 );
  backlightLayout->addWidget( m_brightnessSlider, 1 );
  backlightLayout->addWidget( m_brightnessValueLabel );
  detailsLayout->addWidget( backlightLabel, row, 0 );
  detailsLayout->addLayout( backlightLayout, row, 1 );
  row++;

  QLabel *setBrightnessLabel = new QLabel( "Set brightness on profile activation" );
  m_setBrightnessCheckBox = new QCheckBox();
  m_setBrightnessCheckBox->setChecked( false );
  detailsLayout->addWidget( setBrightnessLabel, row, 0 );
  detailsLayout->addWidget( m_setBrightnessCheckBox, row, 1, Qt::AlignLeft );
  row++;

  m_profileRefreshRate = new QComboBox();
  m_profileRefreshRate->setObjectName("profileRefreshRate");
  m_profileRefreshRate->setSizeAdjustPolicy( QComboBox::AdjustToMinimumContentsLengthWithIcon );
  m_profileRefreshRate->setMinimumContentsLength( 10 );
  rebuildRefreshRateCombo( 0 );
  m_profileRefreshRate->setToolTip("Applies automatically when this profile becomes active. Keeps the current resolution and uses only modes supported by the internal display.");
  detailsLayout->addWidget(new QLabel("Display refresh rate"), row, 0);
  detailsLayout->addWidget(m_profileRefreshRate, row, 1);
  row++;

  m_profileLidOffKbd = new QCheckBox("Turn off keyboard backlight when lid closes");
  m_profileLidOffKbd->setObjectName("turnOffKeyboardBacklightOnLidClose");
  m_profileLidOffKbd->setToolTip("Temporarily sets keyboard zones to brightness 0 while the lid is closed and restores them when reopened.");
  detailsLayout->addWidget(m_profileLidOffKbd, row, 0, 1, 2);
  row++;

  m_profileLidOffWcLed = new QCheckBox("Turn off water cooler LED when lid closes (if connected)");
  m_profileLidOffWcLed->setObjectName("turnOffWaterCoolerLedOnLidClose");
  m_profileLidOffWcLed->setToolTip("Turns off the external water cooler RGB LED while the lid is closed and restores it when reopened.");
  detailsLayout->addWidget(m_profileLidOffWcLed, row, 0, 1, 2);
  row++;

  m_profileLidCloseAction = new QComboBox();
  m_profileLidCloseAction->setObjectName("profileLidCloseAction");
  m_profileLidCloseAction->setSizeAdjustPolicy( QComboBox::AdjustToMinimumContentsLengthWithIcon );
  m_profileLidCloseAction->setMinimumContentsLength( 10 );
  m_profileLidCloseAction->addItem("Default (global)", QString());
  m_profileLidCloseAction->addItem("Turn off screen (keep running)", QStringLiteral("screen_off"));
  m_profileLidCloseAction->addItem("Ignore", QStringLiteral("ignore"));
  m_profileLidCloseAction->addItem("Suspend", QStringLiteral("suspend"));
  m_profileLidCloseAction->addItem("Hibernate", QStringLiteral("hibernate"));
  m_profileLidCloseAction->addItem("Power off", QStringLiteral("poweroff"));
  m_profileLidCloseAction->addItem("Lock screen", QStringLiteral("lock"));
  m_profileLidCloseAction->setToolTip("Overrides the global lid close behavior while this profile is active.");
  detailsLayout->addWidget(new QLabel("Lid close action"), row, 0);
  detailsLayout->addWidget(m_profileLidCloseAction, row, 1);
  row++;

  // Add spacer
  detailsLayout->addItem( new QSpacerItem( 0, 10 ), row, 0, 1, 2 );
  row++;

  // === FAN CONTROL SECTION ===
  QLabel *fanHeader = new QLabel( "Fan control" );
  fanHeader->setStyleSheet( "font-weight: bold; font-size: 14px;" );
  detailsLayout->addWidget( fanHeader, row, 0, 1, 2 );
  row++;

  QLabel *fanProfileLabel = new QLabel( "Fan profile" );
  m_profileFanProfileCombo = new QComboBox();
  m_profileFanProfileCombo->setSizeAdjustPolicy( QComboBox::AdjustToMinimumContentsLengthWithIcon );
  m_profileFanProfileCombo->setMinimumContentsLength( 10 );
  // Add built-in fan profiles from daemon (id + name)
  for ( const auto &v : m_profileManager->builtinFanProfilesData() )
  {
    if ( v.isObject() )
    {
      QJsonObject o = v.toObject();
      m_profileFanProfileCombo->addItem( o["name"].toString(), o["id"].toString() );
    }
  }
  // Append persisted custom fan profiles loaded from settings
  for ( const auto &v : m_profileManager->customFanProfilesData() )
  {
    if ( v.isObject() )
    {
      QJsonObject o = v.toObject();
      QString name = o["name"].toString();
      if ( m_profileFanProfileCombo->findText( name ) == -1 )
        m_profileFanProfileCombo->addItem( name, o["id"].toString() );
    }
  }
  detailsLayout->addWidget( fanProfileLabel, row, 0 );
  detailsLayout->addWidget( m_profileFanProfileCombo, row, 1 );
  row++;

  QLabel *sameSpeedLabel = new QLabel( "Same fan speed for all fans" );
  detailsLayout->addWidget( sameSpeedLabel, row, 0 );
  // Reuse the shared checkbox created in the dashboard (create if not present)
  if ( !m_sameFanSpeedCheckBox ) {
    m_sameFanSpeedCheckBox = new QCheckBox();
    m_sameFanSpeedCheckBox->setChecked( true );
  }
  detailsLayout->addWidget( m_sameFanSpeedCheckBox, row, 1, Qt::AlignLeft );
  row++;

  QLabel *autoWaterLabel = new QLabel( "Water cooler auto control" );
  m_autoWaterControlCheckBox = new QCheckBox();
  m_autoWaterControlCheckBox->setChecked( true );
  m_autoWaterControlCheckBox->setToolTip( tr( "When enabled the daemon will control the water cooler automatically" ) );
  detailsLayout->addWidget( autoWaterLabel, row, 0 );
  detailsLayout->addWidget( m_autoWaterControlCheckBox, row, 1, Qt::AlignLeft );
  row++;

  // Hide water cooler auto control if water cooler not supported
  if ( !m_waterCoolerSupported )
  {
    autoWaterLabel->setVisible( false );
    m_autoWaterControlCheckBox->setVisible( false );
  }

  // Add spacer
  detailsLayout->addItem( new QSpacerItem( 0, 10 ), row, 0, 1, 2 );
  row++;

  // === GPU POWER (cTGP) SECTION ===
  m_ctgpHeader = new QLabel( "GPU Power" );
  m_ctgpHeader->setStyleSheet( "font-weight: bold; font-size: 14px;" );
  detailsLayout->addWidget( m_ctgpHeader, row, 0, 1, 2 );
  row++;

  // cTGP slider widget (hidden when not supported)
  m_ctgpWidget = new QWidget();
  m_ctgpWidget->setObjectName("ctgpWidget");
  QHBoxLayout *ctgpLayout = new QHBoxLayout( m_ctgpWidget );
  ctgpLayout->setContentsMargins( 0, 0, 0, 0 );
  m_ctgpLabel = new QLabel( "Configurable TGP" );
  m_ctgpSlider = new QSlider( Qt::Horizontal );
  m_ctgpSlider->setObjectName("ctgpSlider");
  m_ctgpSlider->setMinimum( 0 );
  m_ctgpSlider->setMaximum( 175 );
  m_ctgpSlider->setValue( 0 );
  m_ctgpValueLabel = new QLabel( "0 W" );
  m_ctgpValueLabel->setMinimumWidth( 50 );
  ctgpLayout->addWidget( m_ctgpSlider, 1 );
  ctgpLayout->addWidget( m_ctgpValueLabel );
  detailsLayout->addWidget( m_ctgpLabel, row, 0 );
  detailsLayout->addWidget( m_ctgpWidget, row, 1 );
  row++;

  updateCtgpVisibility();

  // Add spacer
  detailsLayout->addItem( new QSpacerItem( 0, 10 ), row, 0, 1, 2 );
  row++;

  // === SYSTEM PERFORMANCE SECTION ===
  QLabel *sysHeader = new QLabel( "System performance" );
  sysHeader->setStyleSheet( "font-weight: bold; font-size: 14px;" );
  detailsLayout->addWidget( sysHeader, row, 0, 1, 2 );
  row++;

  QLabel *odmPowerHeader = new QLabel( "CPU power limit control" );
  detailsLayout->addWidget( odmPowerHeader, row, 0, 1, 2 );
  row++;

  // TDP Limit 1
  QLabel *tdp1Label = new QLabel( "Sustained TDP" );  // Sustained Power Limit
  QHBoxLayout *tdp1Layout = new QHBoxLayout();
  m_odmPowerLimit1Slider = new QSlider( Qt::Horizontal );
  m_odmPowerLimit1Slider->setMinimum( 0 );
  m_odmPowerLimit1Slider->setMaximum( 250 );  // Will be updated from hardware limits in loadProfileDetails
  m_odmPowerLimit1Slider->setValue( 0 );
  m_odmPowerLimit1Value = new QLabel( "0 W" );
  m_odmPowerLimit1Value->setMinimumWidth( 50 );
  tdp1Layout->addWidget( m_odmPowerLimit1Slider, 1 );
  tdp1Layout->addWidget( m_odmPowerLimit1Value );
  detailsLayout->addWidget( tdp1Label, row, 0 );
  detailsLayout->addLayout( tdp1Layout, row, 1 );
  row++;

  // TDP Limit 2
  QLabel *tdp2Label = new QLabel( "Boost TDP" );
  QHBoxLayout *tdp2Layout = new QHBoxLayout();
  m_odmPowerLimit2Slider = new QSlider( Qt::Horizontal );
  m_odmPowerLimit2Slider->setMinimum( 0 );
  m_odmPowerLimit2Slider->setMaximum( 250 );  // Will be updated from hardware limits in loadProfileDetails
  m_odmPowerLimit2Slider->setValue( 0 );
  m_odmPowerLimit2Value = new QLabel( "0 W" );
  m_odmPowerLimit2Value->setMinimumWidth( 50 );
  tdp2Layout->addWidget( m_odmPowerLimit2Slider, 1 );
  tdp2Layout->addWidget( m_odmPowerLimit2Value );
  detailsLayout->addWidget( tdp2Label, row, 0 );
  detailsLayout->addLayout( tdp2Layout, row, 1 );
  row++;

  // TDP Limit 3
  QLabel *tdp3Label = new QLabel( "Peak TDP" );
  QHBoxLayout *tdp3Layout = new QHBoxLayout();
  m_odmPowerLimit3Slider = new QSlider( Qt::Horizontal );
  m_odmPowerLimit3Slider->setMinimum( 0 );
  m_odmPowerLimit3Slider->setMaximum( 250 );  // Will be updated from hardware limits in loadProfileDetails
  m_odmPowerLimit3Slider->setValue( 0 );
  m_odmPowerLimit3Value = new QLabel( "0 W" );
  m_odmPowerLimit3Value->setMinimumWidth( 50 );
  tdp3Layout->addWidget( m_odmPowerLimit3Slider, 1 );
  tdp3Layout->addWidget( m_odmPowerLimit3Value );
  detailsLayout->addWidget( tdp3Label, row, 0 );
  detailsLayout->addLayout( tdp3Layout, row, 1 );
  row++;

  // Add spacer
  detailsLayout->addItem( new QSpacerItem( 0, 5 ), row, 0, 1, 2 );
  row++;

  QLabel *cpuFreqHeader = new QLabel( "CPU frequency control" );
  detailsLayout->addWidget( cpuFreqHeader, row, 0, 1, 2 );
  row++;

  QLabel *coresLabel = new QLabel( "Number of logical cores" );
  QHBoxLayout *coresLayout = new QHBoxLayout();
  const int nCores = m_UccdClient->getCpuCoreCount().value_or( 1 );
  m_cpuCoresSlider = new QSlider( Qt::Horizontal );
  m_cpuCoresSlider->setMinimum( 1 );
  m_cpuCoresSlider->setMaximum( nCores > 0 ? nCores : 1 );
  m_cpuCoresSlider->setValue( nCores > 0 ? nCores : 1 );
  m_cpuCoresValue = new QLabel( QString::number( nCores > 0 ? nCores : 1 ) );
  m_cpuCoresValue->setMinimumWidth( 35 );
  coresLayout->addWidget( m_cpuCoresSlider, 1 );
  coresLayout->addWidget( m_cpuCoresValue );
  detailsLayout->addWidget( coresLabel, row, 0 );
  detailsLayout->addLayout( coresLayout, row, 1 );
  row++;
  m_cpuCoreSelection=new CpuCoreSelectionWidget;
  detailsLayout->addWidget(new QLabel("CPU selection"),row,0,Qt::AlignTop);
  detailsLayout->addWidget(m_cpuCoreSelection,row,1);
  row++;

  QLabel *maxPerfLabel = new QLabel( "CPU Governor" );
  m_governorCombo = new QComboBox();
  m_governorCombo->setSizeAdjustPolicy( QComboBox::AdjustToMinimumContentsLengthWithIcon );
  m_governorCombo->setMinimumContentsLength( 10 );
  m_governorCombo->setMinimumWidth( 160 );
  detailsLayout->addWidget( maxPerfLabel, row, 0 );
  detailsLayout->addWidget( m_governorCombo, row, 1, Qt::AlignLeft );
  row++;

  QLabel *eppLabel = new QLabel( "Energy Performance Preference" );
  m_eppCombo = new QComboBox();
  m_eppCombo->setSizeAdjustPolicy( QComboBox::AdjustToMinimumContentsLengthWithIcon );
  m_eppCombo->setMinimumContentsLength( 10 );
  m_eppCombo->setMinimumWidth( 180 );
  detailsLayout->addWidget( eppLabel, row, 0 );
  detailsLayout->addWidget( m_eppCombo, row, 1, Qt::AlignLeft );
  row++;

  QLabel *minFreqLabel = new QLabel( "Minimum frequency" );
  QHBoxLayout *minFreqLayout = new QHBoxLayout();
  m_minFrequencySlider = new QSlider( Qt::Horizontal );
  m_minFrequencySlider->setSingleStep( 100000 ); // 100 MHz steps (in kHz)

  // Get hardware frequency limits and initialize slider with actual values
  int minFreqKHz = 400000;  // fallback
  int maxFreqKHz = 6000000; // fallback
  if ( auto limitsJson = m_UccdClient->getCpuFrequencyLimitsJSON() )
  {
    QJsonDocument doc = QJsonDocument::fromJson( limitsJson->c_str() );
    if ( doc.isObject() )
    {
      QJsonObject limitsObj = doc.object();
      minFreqKHz = limitsObj["min"].toInt( 400000 );
      maxFreqKHz = limitsObj["max"].toInt( 6000000 );
      m_cpuMinFreqKHz = minFreqKHz;
      m_cpuMaxFreqKHz = maxFreqKHz;
    }
  }
  m_minFrequencySlider->setMinimum( minFreqKHz );
  m_minFrequencySlider->setMaximum( maxFreqKHz );
  m_minFrequencySlider->setValue( minFreqKHz );

  m_minFrequencyValue = new QLabel();
  m_minFrequencyValue->setMinimumWidth( 60 );
  double freqGHz = minFreqKHz / 1000000.0;
  m_minFrequencyValue->setText( QString::number( freqGHz, 'f', 2 ) + " GHz" );

  minFreqLayout->addWidget( m_minFrequencySlider, 1 );
  minFreqLayout->addWidget( m_minFrequencyValue );
  detailsLayout->addWidget( minFreqLabel, row, 0 );
  detailsLayout->addLayout( minFreqLayout, row, 1 );
  row++;

  QLabel *maxFreqLabel = new QLabel( "Maximum frequency" );
  QHBoxLayout *maxFreqLayout = new QHBoxLayout();
  m_maxFrequencySlider = new QSlider( Qt::Horizontal );
  m_maxFrequencySlider->setSingleStep( 100000 ); // 100 MHz steps (in kHz)
  m_maxFrequencySlider->setMinimum( minFreqKHz );
  m_maxFrequencySlider->setMaximum( maxFreqKHz );
  m_maxFrequencySlider->setValue( maxFreqKHz );

  m_maxFrequencyValue = new QLabel();
  m_maxFrequencyValue->setMinimumWidth( 60 );
  freqGHz = maxFreqKHz / 1000000.0;
  m_maxFrequencyValue->setText( QString::number( freqGHz, 'f', 2 ) + " GHz" );
  maxFreqLayout->addWidget( m_maxFrequencySlider, 1 );
  maxFreqLayout->addWidget( m_maxFrequencyValue );
  detailsLayout->addWidget( maxFreqLabel, row, 0 );
  detailsLayout->addLayout( maxFreqLayout, row, 1 );
  row++;

  m_hwpDynamicBoostLabel = new QLabel( "HWP Dynamic Boost" );
  m_hwpDynamicBoostCheckBox = new QCheckBox( "Enable" );
  m_hwpDynamicBoostLabel->setVisible( m_hwpDynamicBoostSupported );
  m_hwpDynamicBoostCheckBox->setVisible( m_hwpDynamicBoostSupported );
  detailsLayout->addWidget( m_hwpDynamicBoostLabel, row, 0 );
  detailsLayout->addWidget( m_hwpDynamicBoostCheckBox, row, 1, Qt::AlignLeft );
  row++;

  // Add spacer
  detailsLayout->addItem( new QSpacerItem( 0, 10 ), row, 0, 1, 2 );
  row++;

  detailsLayout->addItem( new QSpacerItem( 0, 20, QSizePolicy::Minimum, QSizePolicy::Expanding ), row, 0, 1, 2 );

  for (auto row : {std::pair<QHBoxLayout *,QLabel *>{backlightLayout,m_brightnessValueLabel},
                   {ctgpLayout,m_ctgpValueLabel},{tdp1Layout,m_odmPowerLimit1Value},
                   {tdp2Layout,m_odmPowerLimit2Value},{tdp3Layout,m_odmPowerLimit3Value},
                   {coresLayout,m_cpuCoresValue},{minFreqLayout,m_minFrequencyValue},
                   {maxFreqLayout,m_maxFrequencyValue}})
    FluentTheme::styleSliderRow(row.first,row.second);
  odmPowerHeader->setObjectName("subheading");
  cpuFreqHeader->setObjectName("subheading");

  // Preserve the original controls and connections, regroup their grid items into cards.
  struct Section { QString name; QFrame *card; QGridLayout *grid; int firstRow; };
  QVector<Section> sections;
  auto startSection=[&](const QString &name,int firstRow){
    auto *card=FluentTheme::createCard();auto *grid=new QGridLayout(card);grid->setContentsMargins(24,24,24,24);grid->setHorizontalSpacing(24);grid->setVerticalSpacing(16);grid->setColumnStretch(1,1);
    sections.append({name,card,grid,firstRow});
  };
  startSection("Profile",0);
  const QStringList sectionNames={"Charging","Display and Keyboard","Fan control","GPU Power","System performance"};
  while(detailsLayout->count()) {
    int r,c,rs,cs;detailsLayout->getItemPosition(0,&r,&c,&rs,&cs);
    auto *item=detailsLayout->takeAt(0);
    if(auto *label=qobject_cast<QLabel*>(item->widget());label && sectionNames.contains(label->text()) && cs==2) {
      startSection(label->text(),r);label->setStyleSheet(QString());label->setObjectName("cardTitle");
    }
    if(item->spacerItem()) {delete item;continue;}
    auto &section=sections.last();
    if(auto *widget=item->widget()){
      if (auto *label=qobject_cast<QLabel *>(widget); label && c==0 && cs==1) label->setWordWrap(true);
      section.grid->addWidget(widget,r-section.firstRow,c,rs,cs,item->alignment());delete item;
      if(widget==m_cpuCoreSelection) {
        section.grid->setSizeConstraint(QLayout::SetMinimumSize);
        const int coreRow=r-section.firstRow;
        auto* grid=section.grid;
        grid->setRowMinimumHeight(coreRow,m_cpuCoreSelection->minimumHeight());
        connect(m_cpuCoreSelection,&CpuCoreSelectionWidget::contentHeightChanged,grid,[grid,coreRow](int height){grid->setRowMinimumHeight(coreRow,height);});
      }
    }
    else if(auto *childLayout=item->layout()){childLayout->setParent(nullptr);section.grid->addLayout(childLayout,r-section.firstRow,c,rs,cs);}
    else {delete item;}
  }
  delete detailsLayout;
  for (auto &section : sections) FluentTheme::collapseWhenEmpty(section.card);
  m_profileCombo->setAccessibleName("Profile name");
  m_profileCombo->setObjectName("profileName");m_profileCombo->setMinimumHeight(36);
  // Follow the reference's emphasis: activation/name, power, charging, cooling/lighting.
  for(const auto &name:QStringList{"Profile","System performance","GPU Power","Charging","Fan control","Display and Keyboard"})
    for(auto &section:sections) if(section.name==name) scrollLayout->addWidget(section.card);
  scrollLayout->addStretch();

  scrollArea->setWidget( scrollWidget );
  mainLayout->addWidget(scrollArea,1);

  m_tabs->addTab( profilesWidget, "Profiles" );
}

void MainWindow::connectSignals()
{
  connect(m_cpuCoreSelection,&CpuCoreSelectionWidget::selectionChanged,this,[this]{
    if(m_cpuCoreSelection->individual()) {
      QJsonObject cpu;m_cpuCoreSelection->writeProfile(cpu);
      m_cpuCoresSlider->setValue(cpu["onlineCores"].toInt());
    }
    markChanged();m_cpuCoresSlider->setEnabled(m_profileManager->isCustomProfile(m_profileCombo->currentData().toString()) && !m_cpuCoreSelection->individual());
  });
  // Profile page connections

  connect( m_profileManager.get(), &ProfileManager::allProfilesChanged,
           this, &MainWindow::onAllProfilesChanged );

  const auto manualCoolerChanged = [this](bool automatic, const QString &message) {
    if (m_currentLoadedProfile == m_profileManager->activeProfileId()) {
      const QSignalBlocker blocker(m_autoWaterControlCheckBox);
      m_autoWaterControlCheckBox->setChecked(automatic);
      m_fanControlTab->setWaterCoolerAutoControl(automatic);
    }
    statusBar()->showMessage(message,5000);
  };
  connect(m_dashboardTab, &DashboardTab::waterCoolerManualChanged, this, manualCoolerChanged);
  connect(m_fanControlTab, &FanControlTab::waterCoolerManualChanged, this, manualCoolerChanged);

  connect( m_profileManager.get(), &ProfileManager::activeProfileIndexChanged,
           this, &MainWindow::onActiveProfileIndexChanged );

  connect( m_profileManager.get(), &ProfileManager::activeProfileChanged,
           this, [this]() {
    if ( m_initializing ) return;  // defer to onAllProfilesChanged after init
    qDebug() << "activeProfileChanged signal received, updating UI";
    loadProfileDetails( m_profileManager->activeProfileId() );
  } );

  connect( m_profileManager.get(), &ProfileManager::customKeyboardProfilesChanged,
           this, &MainWindow::onCustomKeyboardProfilesChanged );

  // Sub-profile sync: when a remote client (e.g. tray) changes the
  // keyboard or fan profile, update the combo boxes and editors without
  // writing back to hardware (the hardware already has the new state).
  connect( m_profileManager.get(), &ProfileManager::activeKeyboardProfileChanged,
           this, [this]( const QString &kbId ) {
    if ( m_initializing ) return;
    updateKeyboardEditorFromProfile( kbId );
  } );
  connect( m_profileManager.get(), &ProfileManager::activeFanProfileChanged,
           this, [this]( const QString &fpId ) {
    if ( m_initializing ) return;
    updateFanEditorFromProfile( fpId );
  } );

  connect( m_profileCombo, QOverload< int >::of( &QComboBox::currentIndexChanged ),
           this, &MainWindow::onProfileIndexChanged );

  // Display controls

  connect( m_brightnessSlider, &QSlider::valueChanged,
           this, &MainWindow::onBrightnessSliderChanged );

  // ODM Power Limit controls

  connect( m_odmPowerLimit1Slider, &QSlider::valueChanged,
           this, &MainWindow::onODMPowerLimit1Changed );

  connect( m_odmPowerLimit2Slider, &QSlider::valueChanged,
           this, &MainWindow::onODMPowerLimit2Changed );

  connect( m_odmPowerLimit3Slider, &QSlider::valueChanged,
           this, &MainWindow::onODMPowerLimit3Changed );

  // CPU frequency controls

  connect( m_cpuCoresSlider, &QSlider::valueChanged,
           this, &MainWindow::onCpuCoresChanged );

  connect( m_maxFrequencySlider, &QSlider::valueChanged,
           this, &MainWindow::onMaxFrequencyChanged );

  connect( m_minFrequencySlider, &QSlider::valueChanged,
           this, [this]( int value ) {
    double freqGHz = value / 1000000.0;  // Convert kHz to GHz for display
    m_minFrequencyValue->setText( QString::number( freqGHz, 'f', 2 ) + " GHz" );
  } );

  // Enforce min <= max for frequency sliders
  connect( m_minFrequencySlider, &QSlider::valueChanged,
           this, [this]( int value ) {
    if ( value > m_maxFrequencySlider->value() )
      m_maxFrequencySlider->setValue( value );
  } );

  connect( m_maxFrequencySlider, &QSlider::valueChanged,
           this, [this]( int value ) {
    if ( value < m_minFrequencySlider->value() )
      m_minFrequencySlider->setValue( value );
  } );

  // Enforce TDP ordering: sustained <= boost <= peak
  connect( m_odmPowerLimit1Slider, &QSlider::valueChanged,
           this, [this]( int value ) {
    if ( value > m_odmPowerLimit2Slider->value() )
      m_odmPowerLimit2Slider->setValue( value );
  } );

  connect( m_odmPowerLimit2Slider, &QSlider::valueChanged,
           this, [this]( int value ) {
    if ( value < m_odmPowerLimit1Slider->value() )
      m_odmPowerLimit1Slider->setValue( value );
    if ( value > m_odmPowerLimit3Slider->value() )
      m_odmPowerLimit3Slider->setValue( value );
  } );

  connect( m_odmPowerLimit3Slider, &QSlider::valueChanged,
           this, [this]( int value ) {
    if ( value < m_odmPowerLimit2Slider->value() )
      m_odmPowerLimit2Slider->setValue( value );
  } );

  // Apply and Save buttons

  connect( m_applyButton, &QPushButton::clicked,
           this, &MainWindow::onApplyClicked );

  connect( m_saveButton, &QPushButton::clicked,
           this, &MainWindow::onSaveClicked );

  connect( m_copyProfileButton, &QPushButton::clicked,
           this, &MainWindow::onCopyProfileClicked );

  connect( m_removeProfileButton, &QPushButton::clicked,
           this, &MainWindow::onRemoveProfileClicked );

  // Connect all profile controls to mark changes

  connect( m_descriptionEdit, &QTextEdit::textChanged,
           this, &MainWindow::markChanged );

  // Profile combo rename handling
  connect( m_profileCombo->lineEdit(), &QLineEdit::editingFinished,
           this, &MainWindow::onProfileComboRenamed );

  connect( m_setBrightnessCheckBox, &QCheckBox::toggled,
           this, &MainWindow::markChanged );
  connect(m_profileRefreshRate, &QComboBox::currentIndexChanged, this, &MainWindow::markChanged);
  if ( m_profileLidOffKbd )
    connect( m_profileLidOffKbd, &QCheckBox::toggled, this, &MainWindow::markChanged );
  if ( m_profileLidOffWcLed )
    connect( m_profileLidOffWcLed, &QCheckBox::toggled, this, &MainWindow::markChanged );
  if ( m_profileLidCloseAction )
    connect( m_profileLidCloseAction, &QComboBox::currentIndexChanged, this, &MainWindow::markChanged );

  connect( m_brightnessSlider, &QSlider::valueChanged,
           this, [this]() { markChanged(); } );

  connect( m_profileFanProfileCombo, QOverload< int >::of( &QComboBox::currentIndexChanged ),
           this, [this](int index) {
             markChanged();
             const QString fpId = m_profileFanProfileCombo->itemData(index).toString();
             // Update fan profile tab to match profile tab selection when visible in current filter
             m_fanControlTab->fanProfileCombo()->blockSignals(true);
             const int tabIdx = m_fanControlTab->fanProfileCombo()->findData(fpId);
             if (tabIdx >= 0)
               m_fanControlTab->fanProfileCombo()->setCurrentIndex(tabIdx);
             m_fanControlTab->fanProfileCombo()->blockSignals(false);
             // Load the fan curves for the new profile
             onFanProfileChanged(fpId);
           } );

  connect( m_autoWaterControlCheckBox, &QCheckBox::toggled,
           this, &MainWindow::markChanged );
  connect( m_autoWaterControlCheckBox, &QCheckBox::toggled,
           this, [this]( bool autoControl ) {
             if ( m_fanControlTab )
               m_fanControlTab->setWaterCoolerAutoControl( autoControl );
           } );

  connect( m_cpuCoresSlider, &QSlider::valueChanged,
           this, [this]() { markChanged(); } );

  connect( m_governorCombo, QOverload<int>::of(&QComboBox::currentIndexChanged),
           this, &MainWindow::markChanged );

  connect( m_eppCombo, QOverload<int>::of(&QComboBox::currentIndexChanged),
           this, &MainWindow::markChanged );

  if ( m_hwpDynamicBoostCheckBox )
    connect( m_hwpDynamicBoostCheckBox, &QCheckBox::toggled,
             this, [this]() { markChanged(); } );

  connect( m_minFrequencySlider, &QSlider::valueChanged,
           this, [this]() { markChanged(); } );

  connect( m_maxFrequencySlider, &QSlider::valueChanged,
           this, [this]() { markChanged(); } );

  connect( m_odmPowerLimit1Slider, &QSlider::valueChanged,
           this, [this]() { markChanged(); } );

  connect( m_odmPowerLimit2Slider, &QSlider::valueChanged,
           this, [this]() { markChanged(); } );

  connect( m_odmPowerLimit3Slider, &QSlider::valueChanged,
           this, [this]() { markChanged(); } );

  // cTGP slider connection
  if ( m_ctgpSlider )
  {
    connect( m_ctgpSlider, &QSlider::valueChanged,
             this, &MainWindow::onCtgpSliderChanged );
    connect( m_ctgpSlider, &QSlider::valueChanged,
             this, [this]() { markChanged(); } );
  }

  connect( m_profileKeyboardProfileCombo, QOverload< int >::of( &QComboBox::currentIndexChanged ),
           this, [this](int index) {
             markChanged();
             // Keep the keyboard tab combo in sync so it reflects the selection,
             // but do NOT apply to hardware - that happens when the profile is saved/applied.
             m_keyboardProfileCombo->blockSignals(true);
             m_keyboardProfileCombo->setCurrentIndex(index);
             m_keyboardProfileCombo->blockSignals(false);
           } );


  // Mains/Battery/Water Cooler activation buttons
  connect(m_typeCButton, &QPushButton::toggled, this, &MainWindow::markChanged);
  connect( m_mainsButton, &QPushButton::toggled,
           this, &MainWindow::markChanged );

  connect( m_batteryButton, &QPushButton::toggled,
           this, &MainWindow::markChanged );

  connect( m_waterCoolerButton, &QPushButton::toggled,
           this, &MainWindow::markChanged );

  // Error handling

  connect( m_profileManager.get(), QOverload< const QString & >::of( &ProfileManager::error ),
           this, [this]( const QString &msg ) {
    statusBar()->showMessage( "Error: " + msg );
    m_saveInProgress = false;
    updateButtonStates();
  } );

  // React to DBus client connection status so we can (re)load built-in fan profiles
  connect( m_UccdClient.get(), &UccdClient::connectionStatusChanged,
           this, &MainWindow::onUccdConnectionChanged );

  // Update connection status label when active profile changes
  connect( m_profileManager.get(), &ProfileManager::activeProfileIndexChanged,
           this, &MainWindow::updateConnectionStatusLabel );

  connectKeyboardBacklightPageWidgets();

  // Sync profile page keyboard combo (profiles may have been added before
  // the customKeyboardProfilesChanged signal was connected)
  onCustomKeyboardProfilesChanged();

  // Initial load of fan profiles (may be empty if service not yet available)
  reloadFanProfiles();

  // Populate governor combo
  populateGovernorCombo();

  // Populate EPP combo
  populateEppCombo();

  // Live crosshair tracking on fan curve editors (once per telemetry tick)
  connect( m_systemMonitor.get(), &SystemMonitor::metricsUpdated,              this, &MainWindow::updateFanCrosshairs );
}

void MainWindow::populateGovernorCombo()
{
  if ( !m_governorCombo )
    return;

  m_governorCombo->clear();

  if ( auto governors = m_UccdClient->getAvailableCpuGovernors(); governors && !governors->empty() )
  {
    for ( const auto &gov : *governors )
      m_governorCombo->addItem( QString::fromStdString( gov ), QString::fromStdString( gov ) );
  }
}

void MainWindow::populateEppCombo()
{
  if ( !m_eppCombo )
    return;

  m_eppCombo->clear();

  if ( auto epps = m_UccdClient->getAvailableEPPs(); epps && !epps->empty() )
  {
    for ( const auto &epp : *epps )
      m_eppCombo->addItem( QString::fromStdString( epp ), QString::fromStdString( epp ) );
  }
}

std::optional< double > MainWindow::parseMonitorValue( const QString &str )
{
  QString s = str.trimmed();
  if ( s == "--" || s.isEmpty() )
    return std::nullopt;

  // Strip known suffixes (use QString for correct QChar-level chop)
  static const QStringList suffixes = { QStringLiteral( "°C" ),
                                        QStringLiteral( " %" ),
                                        QStringLiteral( " MHz" ),
                                        QStringLiteral( " W" ),
                                        QStringLiteral( " RPM" ) };
  for ( const auto &suffix : suffixes )
  {
    if ( s.endsWith( suffix ) )
    {
      s.chop( suffix.size() );
      break;
    }
  }

  bool ok = false;
  double v = s.toDouble( &ok );
  return ok ? std::optional< double >( v ) : std::nullopt;
}

void MainWindow::updateFanCrosshairs()
{
  const int fanTabIndex = m_tabs->indexOf( m_fanControlTab );
  if ( !m_fanControlTab || m_tabs->currentIndex() != fanTabIndex )
    return;

  // CPU fan editor
  {
    auto temp = parseMonitorValue( m_systemMonitor->cpuTemp() );
    auto duty = parseMonitorValue( m_systemMonitor->cpuFanSpeed() );
    qDebug() << "[Crosshair] CPU raw:" << m_systemMonitor->cpuTemp() << m_systemMonitor->cpuFanSpeed()
             << "parsed temp:" << ( temp ? *temp : -1 ) << "duty:" << ( duty ? *duty : -1 );
    if ( temp && duty )
      m_fanControlTab->cpuEditor()->setCrosshair( *temp, *duty );
    else
      m_fanControlTab->cpuEditor()->clearCrosshair();
  }

  // GPU fan editor
  {
    auto temp = parseMonitorValue( m_systemMonitor->gpuTemp() );
    auto duty = parseMonitorValue( m_systemMonitor->gpuFanSpeed() );
    if ( temp && duty )
      m_fanControlTab->gpuEditor()->setCrosshair( *temp, *duty );
    else
      m_fanControlTab->gpuEditor()->clearCrosshair();
  }

  // Water cooler fan editor (uses CPU temp as temperature source)
  if ( m_fanControlTab->wcFanEditor() )
  {
    auto temp = parseMonitorValue( m_systemMonitor->cpuTemp() );
    auto duty = parseMonitorValue( m_systemMonitor->waterCoolerFanSpeed() );
    if ( temp && duty )
      m_fanControlTab->wcFanEditor()->setCrosshair( *temp, *duty );
    else
      m_fanControlTab->wcFanEditor()->clearCrosshair();
  }

  // Pump curve editor (uses CPU temp and pump voltage level)
  if ( m_fanControlTab->pumpEditor() )
  {
    auto temp = parseMonitorValue( m_systemMonitor->cpuTemp() );
    int pumpLevel = -1;
    const QString &lvlStr = m_systemMonitor->waterCoolerPumpLevel();
    if ( lvlStr == "Off" )       pumpLevel = 0;
    else if ( lvlStr == "Low" )  pumpLevel = 1;
    else if ( lvlStr == "Med" )  pumpLevel = 2;
    else if ( lvlStr == "High" || lvlStr == "Max" ) pumpLevel = 3;

    if ( temp && pumpLevel >= 0 )
      m_fanControlTab->pumpEditor()->setCrosshair( *temp, pumpLevel );
    else
      m_fanControlTab->pumpEditor()->clearCrosshair();
  }
}

void MainWindow::onTabChanged( int index )
{
  const int fanTabIndex = m_fanControlTab ? m_tabs->indexOf( m_fanControlTab ) : -1;
  updateMonitoringActivity();

  // Update or clear fan curve crosshairs
  if ( index == fanTabIndex )
    updateFanCrosshairs();
  else if ( m_fanControlTab )
  {
    m_fanControlTab->cpuEditor()->clearCrosshair();
    m_fanControlTab->gpuEditor()->clearCrosshair();
    if ( m_fanControlTab->wcFanEditor() )
      m_fanControlTab->wcFanEditor()->clearCrosshair();
    if ( m_fanControlTab->pumpEditor() )
      m_fanControlTab->pumpEditor()->clearCrosshair();
  }

  // Load current keyboard backlight states when Lighting -> Keyboard tab is active
  if ( index == 4 && ( !m_lightingTabs || m_lightingTabs->currentIndex() == 0 ) )
  {
    refreshKeyboardLightingState();
  }

  // Experimental tab: refresh read-only MUX state and enforce risk-acceptance gate
  if ( m_experimentalPage && m_tabs && m_tabs->widget( index ) == m_experimentalPage )
  {
    refreshMuxState();
    updateExperimentalLockState();
    if ( !m_experimentalRisksAccepted && !m_experimentalPromptOpen )
    {
      m_experimentalPromptOpen = true;
      QTimer::singleShot( 0, this, [this]() {
        m_experimentalPromptOpen = false;
        if ( m_tabs && m_tabs->currentWidget() == m_experimentalPage )
          promptExperimentalRisks();
      } );
    }
  }
  else if ( m_experimentalRiskDialog )
  {
    m_experimentalRiskDialog->reject();
  }
}

void MainWindow::updateExperimentalLockState()
{
  if ( m_experimentalUnlockButton )
    m_experimentalUnlockButton->setVisible( !m_experimentalRisksAccepted );
  if ( m_experimentalOptionsCard )
    m_experimentalOptionsCard->setEnabled( m_experimentalRisksAccepted );
  if ( m_muxBootModeCombo )
    m_muxBootModeCombo->setEnabled( m_experimentalRisksAccepted && m_muxAvailable && !readOnlyPreview );
  if ( m_muxApplyButton )
    m_muxApplyButton->setEnabled( m_experimentalRisksAccepted && m_muxAvailable && !readOnlyPreview );
  if ( m_muxReloadButton )
    m_muxReloadButton->setEnabled( m_experimentalRisksAccepted );
}

void MainWindow::promptExperimentalRisks()
{
  if ( m_experimentalRisksAccepted || m_experimentalPromptOpen )
    return;
  m_experimentalPromptOpen = true;

  auto *dlg = new QDialog( this );
  m_experimentalRiskDialog = dlg;
  dlg->setAttribute( Qt::WA_DeleteOnClose );
  dlg->setWindowTitle( "Experimental Features Warning" );
  dlg->setModal( true );
  dlg->setMinimumWidth( 440 );

  auto *layout = new QVBoxLayout( dlg );
  layout->setContentsMargins( 20, 18, 20, 18 );
  layout->setSpacing( 14 );

  auto *heading = new QLabel( "Experimental & Unstable Features", dlg );
  heading->setObjectName( "cardTitle" );
  layout->addWidget( heading );

  auto *body = new QLabel(
      "The settings in the Experimental tab are experimental and unstable. "
      "Changing firmware-level options such as the Display MUX Switch may cause display issues, "
      "unexpected behavior, or require resetting BIOS settings.\n\n"
      "No Experimental setting will work until you explicitly accept these risks.",
      dlg );
  body->setWordWrap( true );
  layout->addWidget( body );

  auto *dontShowAgain = new QCheckBox( "Don't show again", dlg );
  dontShowAgain->setObjectName( "experimentalDontShowAgain" );
  layout->addWidget( dontShowAgain );

  auto *buttons = new QHBoxLayout();
  buttons->addStretch( 1 );
  auto *cancelBtn = new QPushButton( "Cancel", dlg );
  auto *acceptBtn = new QPushButton( "I accept the risks", dlg );
  acceptBtn->setObjectName( "experimentalAcceptRisks" );
  acceptBtn->setProperty( "primary", true );
  buttons->addWidget( cancelBtn );
  buttons->addWidget( acceptBtn );
  layout->addLayout( buttons );

  connect( cancelBtn, &QPushButton::clicked, dlg, &QDialog::reject );
  connect( acceptBtn, &QPushButton::clicked, dlg, &QDialog::accept );
  connect( dlg, &QDialog::finished, this, [this, dontShowAgain]( int result ) {
    m_experimentalPromptOpen = false;
    if ( result == QDialog::Accepted )
    {
      m_experimentalRisksAccepted = true;
      if ( dontShowAgain->isChecked() && !readOnlyPreview )
      {
        QSettings( "UniwillControlCenter", "desktop" )
            .setValue( "experimentalRisksAcceptedPermanently", true );
      }
    }
    updateExperimentalLockState();
  } );

  dlg->open();
}

void MainWindow::refreshMuxState()
{
  if ( !m_UccdClient || !m_muxActiveGpuLabel || !m_muxBootModeCombo || !m_muxStatusLabel )
    return;
  const auto json = m_UccdClient->getMuxStateJSON();
  const auto doc = json ? QJsonDocument::fromJson( QByteArray::fromStdString( *json ) ) : QJsonDocument();
  const QJsonObject obj = doc.isObject() ? doc.object() : QJsonObject();
  m_muxAvailable = obj["available"].toBool( false );
  if ( !m_muxAvailable )
  {
    m_muxActiveGpuLabel->setText( "Unavailable" );
    m_muxStatusLabel->setText( "Hardware Display MUX (uwcc ACPI MXDM/MXDS) is not supported or daemon is disconnected." );
    updateExperimentalLockState();
    return;
  }
  const QString activeGpu = obj["activeGpu"].toString( "unknown" );
  const QString bootMode = obj["bootMode"].toString( "unknown" );
  m_muxActiveGpuLabel->setText( activeGpu == "dgpu" ? "Discrete GPU (dGPU)"
                               : activeGpu == "igpu" ? "Integrated GPU (iGPU)"
                                                     : "Unknown" );
  const QSignalBlocker blocked( m_muxBootModeCombo );
  const int idx = m_muxBootModeCombo->findData( bootMode );
  if ( idx >= 0 )
    m_muxBootModeCombo->setCurrentIndex( idx );
  m_muxStatusLabel->setText( QString( "Current BIOS boot mode: %1" ).arg(
      idx >= 0 ? m_muxBootModeCombo->itemText( idx ) : bootMode ) );
  updateExperimentalLockState();
}

void MainWindow::applyMuxBootMode()
{
  if ( !m_experimentalRisksAccepted || readOnlyPreview || !m_muxAvailable || !m_UccdClient || !m_muxBootModeCombo )
    return;
  const QString mode = m_muxBootModeCombo->currentData().toString();
  if ( mode.isEmpty() )
    return;
  if ( m_UccdClient->setMuxBootMode( mode.toStdString() ) )
  {
    statusBar()->showMessage( "Display MUX boot mode updated — reboot to apply", 5000 );
    if ( m_muxStatusLabel )
      m_muxStatusLabel->setText( QString( "Saved BIOS boot mode: %1 (reboot required)" ).arg( m_muxBootModeCombo->currentText() ) );
  }
  else
  {
    statusBar()->showMessage( "Failed to update Display MUX boot mode", 5000 );
    if ( m_muxStatusLabel )
      m_muxStatusLabel->setText( "Failed to write Display MUX boot mode via uwcc." );
  }
}

void MainWindow::refreshKeyboardLightingState()
{
  if ( m_keyboardVisualizer )
  {
    if ( auto states = m_UccdClient->getKeyboardBacklightStates() )
    {
      // Block visualizer signals to prevent hardware write during state refresh
      m_keyboardVisualizer->blockSignals( true );
      m_keyboardVisualizer->loadCurrentStates( *states );

      // Read brightness from live hardware states and sync slider without overwriting hardware
      if ( QJsonDocument statesDoc = QJsonDocument::fromJson( QString::fromStdString( *states ).toUtf8() );
           statesDoc.isArray() && !statesDoc.array().isEmpty() )
      {
        const int hwBrightness = statesDoc.array()[0].toObject()["brightness"].toInt( -1 );
        if ( hwBrightness >= 0 && m_keyboardBrightnessSlider )
        {
          m_keyboardBrightnessSlider->blockSignals( true );
          m_keyboardBrightnessSlider->setValue( hwBrightness );
          m_keyboardBrightnessSlider->blockSignals( false );
          updateKeyboardAppearanceLabels();
        }
      }
      m_keyboardVisualizer->blockSignals( false );
    }
  }

  // Reload keyboard profiles and reflect the active profile selection without pushing stale
  // profile brightness/colors over live hardware state.
  reloadKeyboardProfiles();
  if ( QString activeProfileId = m_profileManager->activeProfileId(); !activeProfileId.isEmpty() )
  {
    if ( QString profileJson = m_profileManager->getProfileDetails( activeProfileId );
         !profileJson.isEmpty() )
    {
      if ( QJsonDocument doc = QJsonDocument::fromJson( profileJson.toUtf8() ); doc.isObject() )
      {
        const QString keyboardProfileId = doc.object()["selectedKeyboardProfile"].toString();
        if ( !keyboardProfileId.isEmpty() && m_keyboardProfileCombo )
        {
          for ( int i = 0; i < m_keyboardProfileCombo->count(); ++i )
          {
            if ( m_keyboardProfileCombo->itemData( i ).toString() == keyboardProfileId )
            {
              m_keyboardProfileCombo->blockSignals( true );
              m_keyboardProfileCombo->setCurrentIndex( i );
              m_keyboardProfileCombo->blockSignals( false );
              break;
            }
          }
        }
      }
    }
  }
}

// Profile page slots
void MainWindow::onCustomKeyboardProfilesChanged()
{
  // Remember current selection so we can restore it after rebuild
  QString prevId = m_profileKeyboardProfileCombo->currentData().toString();

  // Repopulate keyboard profile combos with ID userData
  m_profileKeyboardProfileCombo->blockSignals( true );
  m_profileKeyboardProfileCombo->clear();
  for ( const auto &v : m_profileManager->customKeyboardProfilesData() )
  {
    if ( v.isObject() )
    {
      QJsonObject o = v.toObject();
      m_profileKeyboardProfileCombo->addItem( o["name"].toString(), o["id"].toString() );
    }
  }

  // Restore previous selection by ID
  if ( !prevId.isEmpty() )
  {
    for ( int i = 0; i < m_profileKeyboardProfileCombo->count(); ++i )
    {
      if ( m_profileKeyboardProfileCombo->itemData( i ).toString() == prevId )
      { m_profileKeyboardProfileCombo->setCurrentIndex( i ); break; }
    }
  }
  m_profileKeyboardProfileCombo->blockSignals( false );

  reloadKeyboardProfiles();
}

void MainWindow::onProfileIndexChanged( int index )
{
  if ( index >= 0 )
  {
    QString profileName = m_profileCombo->currentText();
    QString profileId = m_profileCombo->currentData().toString();
    qDebug() << "Profile selected:" << profileName << "at index" << index;
    m_selectedProfileIndex = index;
    loadProfileDetails( profileId );
    m_removeProfileButton->setEnabled( m_profileManager->isCustomProfile( profileId ) );
    m_copyProfileButton->setEnabled( true );
    m_saveButton->setEnabled( true );
    statusBar()->showMessage( "Profile selected: " + profileName + " (click Apply to activate)" );
    if(m_entrance && m_tabs->currentIndex()==1) m_entrance->play();
  }
}

void MainWindow::onAllProfilesChanged()
{
  // Block combo signals to prevent cascading loadProfileDetails calls
  // while we repopulate the list.
  m_profileCombo->blockSignals( true );
  m_profileCombo->clear();
  // Populate combo with name + ID userData
  const QJsonArray &defaultData = m_profileManager->defaultProfilesData();
  const QJsonArray &customData = m_profileManager->customProfilesData();
  for ( const auto &p : defaultData )
  {
    if ( p.isObject() )
      m_profileCombo->addItem( p.toObject()["name"].toString(), p.toObject()["id"].toString() );
  }
  for ( const auto &p : customData )
  {
    if ( p.isObject() )
      m_profileCombo->addItem( p.toObject()["name"].toString(), p.toObject()["id"].toString() );
  }
  m_profileCombo->setCurrentIndex( m_profileManager->activeProfileIndex() );
  m_profileCombo->blockSignals( false );
  m_selectedProfileIndex = m_profileManager->activeProfileIndex();

  // Load the active profile details
  if ( QString activeProfileId = m_profileManager->activeProfileId(); !activeProfileId.isEmpty() )
  {
    loadProfileDetails( activeProfileId );
  }
  // Custom profiles may have changed; reload fan profiles (adds custom entries to the fan combo)
  reloadFanProfiles();

  // Override stored keyboard/fan profile with the daemon's live state.
  // A remote client (e.g. tray applet) may have changed the sub-profile
  // while the GUI was not running or the ProfileChanged signal may have
  // arrived with updated sub-profile IDs.
  {
    QString liveKbId = m_profileManager->activeKeyboardProfileId();
    QString liveFpId = m_profileManager->activeFanProfileId();
    if ( !liveKbId.isEmpty() )
      updateKeyboardEditorFromProfile( liveKbId );
    if ( !liveFpId.isEmpty() )
      updateFanEditorFromProfile( liveFpId );
  }

  // If we were in the middle of saving, mark as complete
  if ( m_saveInProgress )
  {
    m_saveInProgress = false;
    m_profileChanged = false;
    statusBar()->showMessage( "Profile saved successfully" );
    updateButtonStates();
  }

  // Ensure buttons reflect current profile set (remove button availability etc.)
  updateButtonStates();
  m_saveButton->setEnabled( true );
}

void MainWindow::updateSidebarConnectionStatus( bool connected )
{
  if ( !m_sidebarConnectionLabel || !m_sidebarConnectionIndicator )
    return;

  if ( connected )
  {
    m_resumingFromSleep = false;
    if ( m_resumeGraceTimer )
      m_resumeGraceTimer->stop();
    m_sidebarConnectionLabel->setText( QStringLiteral( "Service connected" ) );
    m_sidebarConnectionIndicator->setText( QStringLiteral( "●" ) );
    m_sidebarConnectionIndicator->setProperty( "status", QStringLiteral( "success" ) );
  }
  else if ( m_resumingFromSleep )
  {
    m_sidebarConnectionLabel->setText( QStringLiteral( "Resuming from sleep…" ) );
    m_sidebarConnectionIndicator->setText( QStringLiteral( "○" ) );
    m_sidebarConnectionIndicator->setProperty( "status", QStringLiteral( "warning" ) );
  }
  else
  {
    m_sidebarConnectionLabel->setText( QStringLiteral( "Service unavailable" ) );
    m_sidebarConnectionIndicator->setText( QStringLiteral( "○" ) );
    m_sidebarConnectionIndicator->setProperty( "status", QStringLiteral( "muted" ) );
  }

  m_sidebarConnectionIndicator->style()->unpolish( m_sidebarConnectionIndicator );
  m_sidebarConnectionIndicator->style()->polish( m_sidebarConnectionIndicator );
}

void MainWindow::updateConnectionStatusLabel()
{
  if ( !m_connectionLabel )
    return;

  // Only update if currently connected
  if ( !m_UccdClient->isConnected() )
    return;

  QString profileName = m_profileManager->activeProfileName();
  if ( profileName.isEmpty() )
    profileName = QStringLiteral( "Unknown" );

  m_connectionLabel->setText(
    QString("Profile: %1").arg(profileName) );
}

void MainWindow::onPrepareForSleep( bool goingToSleep )
{
  if ( goingToSleep )
    return;
  m_resumingFromSleep = true;
  if ( m_resumeGraceTimer )
    m_resumeGraceTimer->start();
  if ( !m_UccdClient->isConnected() )
    updateSidebarConnectionStatus( false );
  if ( m_connectionLabel )
    m_connectionLabel->setText( QStringLiteral( "Resuming from sleep…" ) );
  statusBar()->showMessage( QStringLiteral( "Resuming from sleep…" ), 5000 );
  if ( m_profileManager )
    m_profileManager->refresh();
}

void MainWindow::onActiveProfileIndexChanged()
{
  int activeIndex = m_profileManager->activeProfileIndex();
  if ( m_profileCombo->currentIndex() != activeIndex )
  {
    m_profileCombo->blockSignals( true );
    m_profileCombo->setCurrentIndex( activeIndex );
    m_profileCombo->blockSignals( false );
  }
  m_selectedProfileIndex = activeIndex;
}

// Profile detail control slot implementations
void MainWindow::onBrightnessSliderChanged( int value )
{
  m_brightnessValueLabel->setText( QString::number( value ) + "%" );
}

void MainWindow::onCpuCoresChanged( int value )
{
  if(m_cpuCoreSelection) m_cpuCoreSelection->setLegacyCount(value);
  m_cpuCoresValue->setText( QString::number( value ) );
}

// Sub-profile sync helpers: update combo + editor without writing to hardware

void MainWindow::updateKeyboardEditorFromProfile( const QString &keyboardProfileId )
{
  // Update keyboard tab combo
  if ( m_keyboardProfileCombo )
  {
    m_keyboardProfileCombo->blockSignals( true );
    for ( int i = 0; i < m_keyboardProfileCombo->count(); ++i )
    {
      if ( m_keyboardProfileCombo->itemData( i ).toString() == keyboardProfileId )
      {
        m_keyboardProfileCombo->setCurrentIndex( i );
        break;
      }
    }
    m_keyboardProfileCombo->blockSignals( false );
  }

  // Update profile-page keyboard combo
  if ( m_profileKeyboardProfileCombo )
  {
    m_profileKeyboardProfileCombo->blockSignals( true );
    for ( int i = 0; i < m_profileKeyboardProfileCombo->count(); ++i )
    {
      if ( m_profileKeyboardProfileCombo->itemData( i ).toString() == keyboardProfileId )
      {
        m_profileKeyboardProfileCombo->setCurrentIndex( i );
        break;
      }
    }
    m_profileKeyboardProfileCombo->blockSignals( false );
  }

  // Update the keyboard visualizer without writing to hardware
  QString json = m_profileManager->getKeyboardProfile( keyboardProfileId );
  if ( json.isEmpty() || json == "{}" )
    return;

  QJsonDocument doc = QJsonDocument::fromJson( json.toUtf8() );
  if ( !doc.isObject() )
    return;

  QJsonObject obj = doc.object();
  int brightness = obj.value( "brightness" ).toInt( -1 );
  QJsonArray statesArray = obj.value( "states" ).toArray();

  if ( !statesArray.isEmpty() && m_keyboardVisualizer )
  {
    m_keyboardVisualizer->blockSignals( true );
    m_keyboardVisualizer->updateFromJSON( statesArray );
    m_keyboardVisualizer->blockSignals( false );
  }

  if ( brightness >= 0 && m_keyboardBrightnessSlider )
  {
    m_keyboardBrightnessSlider->blockSignals( true );
    m_keyboardBrightnessSlider->setValue( brightness );
    m_keyboardBrightnessSlider->blockSignals( false );
  }

  updateKeyboardAppearanceLabels();
  updateKeyboardProfileButtonStates();
}

void MainWindow::updateFanEditorFromProfile( const QString &fanProfileId )
{
  // Update fan tab combo
  if ( m_fanControlTab && m_fanControlTab->fanProfileCombo() )
  {
    auto *combo = m_fanControlTab->fanProfileCombo();
    combo->blockSignals( true );
    for ( int i = 0; i < combo->count(); ++i )
    {
      if ( combo->itemData( i ).toString() == fanProfileId )
      {
        combo->setCurrentIndex( i );
        break;
      }
    }
    combo->blockSignals( false );
  }

  // Update profile-page fan combo
  if ( m_profileFanProfileCombo )
  {
    m_profileFanProfileCombo->blockSignals( true );
    for ( int i = 0; i < m_profileFanProfileCombo->count(); ++i )
    {
      if ( m_profileFanProfileCombo->itemData( i ).toString() == fanProfileId )
      {
        m_profileFanProfileCombo->setCurrentIndex( i );
        break;
      }
    }
    m_profileFanProfileCombo->blockSignals( false );
  }

  // Load fan curve data into editors without writing to hardware
  onFanProfileChanged( fanProfileId );
}

void MainWindow::onMaxFrequencyChanged( int value )
{
  double freqGHz = value / 1000000.0;  // Convert kHz to GHz for display
  m_maxFrequencyValue->setText( QString::number( freqGHz, 'f', 2 ) + " GHz" );
}

void MainWindow::onODMPowerLimit1Changed( int value )
{
  m_odmPowerLimit1Value->setText( QString::number( value ) + " W" );
}

void MainWindow::onODMPowerLimit2Changed( int value )
{
  m_odmPowerLimit2Value->setText( QString::number( value ) + " W" );
}

void MainWindow::onODMPowerLimit3Changed( int value )
{
  m_odmPowerLimit3Value->setText( QString::number( value ) + " W" );
}

void MainWindow::onCtgpSliderChanged( int value )
{
  m_ctgpValueLabel->setText( QString::number( value ) + " W" );
}

void MainWindow::loadProfileDetails( const QString &profileId )
{
  // Reset change flag when loading a new profile
  m_profileChanged = false;
  m_currentLoadedProfile = profileId;
  updateButtonStates();


  if ( profileId.isEmpty() )
  {
    return;
  }

  // Get the profile JSON from ProfileManager using the profile ID
  QString profileJson = m_profileManager->getProfileDetails( profileId );


  if ( profileJson.isEmpty() )
  {
    return;
  }

  QJsonDocument doc = QJsonDocument::fromJson( profileJson.toUtf8() );

  if ( !doc.isObject() )
  {
    return;
  }

  QJsonObject obj = doc.object();
  if (profileId == m_profileManager->activeProfileId()) {
    if (auto runtime = m_UccdClient->getActiveProfileJSON()) {
      const auto runtimeFan = QJsonDocument::fromJson(QByteArray::fromStdString(*runtime)).object()["fan"].toObject();
      if (runtimeFan.contains("autoControlWC")) {
        auto fan = obj["fan"].toObject();fan["autoControlWC"] = runtimeFan["autoControlWC"];obj["fan"] = fan;
      }
    }
  }
  // Block signals while updating to avoid triggering slot updates
  m_brightnessSlider->blockSignals( true );
  m_setBrightnessCheckBox->blockSignals( true );
  m_profileRefreshRate->blockSignals(true);
  m_profileFanProfileCombo->blockSignals( true );
  m_fanControlTab->fanProfileCombo()->blockSignals( true );
  if ( m_autoWaterControlCheckBox ) m_autoWaterControlCheckBox->blockSignals( true );
  m_cpuCoresSlider->blockSignals( true );
  m_governorCombo->blockSignals( true );
  m_minFrequencySlider->blockSignals( true );
  m_maxFrequencySlider->blockSignals( true );
  m_odmPowerLimit1Slider->blockSignals( true );
  m_odmPowerLimit2Slider->blockSignals( true );
  m_odmPowerLimit3Slider->blockSignals( true );
  m_profileKeyboardProfileCombo->blockSignals( true );
  m_keyboardProfileCombo->blockSignals( true );
  if ( m_profileLidOffKbd ) m_profileLidOffKbd->blockSignals( true );
  if ( m_profileLidOffWcLed ) m_profileLidOffWcLed->blockSignals( true );
  if ( m_profileLidCloseAction ) m_profileLidCloseAction->blockSignals( true );
  m_mainsButton->blockSignals( true );
  m_typeCButton->blockSignals(true);
  m_batteryButton->blockSignals( true );
  m_waterCoolerButton->blockSignals( true );
  if ( m_descriptionEdit ) m_descriptionEdit->blockSignals( true );

  // Load Display settings (nested in display object)
  const auto display = obj["display"].toObject();
  const int refreshHz = display["useRefRate"].toBool() ? display["refreshRate"].toInt() : 0;
  rebuildRefreshRateCombo( refreshHz );
  if ( m_profileLidCloseAction )
  {
    const QString lidAction = display.value( QStringLiteral( "lidCloseAction" ) ).toString();
    const int lidIdx = m_profileLidCloseAction->findData( lidAction );
    m_profileLidCloseAction->setCurrentIndex( lidIdx >= 0 ? lidIdx : 0 );
  }

  if ( obj.contains( "display" ) && obj["display"].isObject() )
  {
    QJsonObject displayObj = obj["display"].toObject();


    if ( displayObj.contains( "brightness" ) )
    {
      int brightness = displayObj["brightness"].toInt( 100 );
      m_brightnessSlider->setValue( brightness );
    }


    if ( displayObj.contains( "useBrightness" ) )
    {
      bool useBrightness = displayObj["useBrightness"].toBool( false );
      m_setBrightnessCheckBox->setChecked( useBrightness );
    }
  }

  // Load Fan Control settings (nested in fan object)

  // Load description
  if ( m_descriptionEdit )
    m_descriptionEdit->setPlainText( obj["description"].toString() );

  QString loadedFanProfile;
  bool fanProfileNotFound = false;
  QString missingFanProfile;
  if ( obj.contains( "fan" ) && obj["fan"].isObject() )
  {
    QJsonObject fanObj = obj["fan"].toObject();


    if ( fanObj.contains( "fanProfile" ) )
    {
      QString fanProfileRef = fanObj["fanProfile"].toString( "fan-balanced" );
      int idx = -1;
      for ( int i = 0; i < m_profileFanProfileCombo->count(); ++i )
      {
        if ( m_profileFanProfileCombo->itemData( i ).toString() == fanProfileRef )
        {
          idx = i;
          break;
        }
      }

      if ( idx >= 0 )
      {
        m_profileFanProfileCombo->setCurrentIndex( idx );
        const int tabIdx = m_fanControlTab->fanProfileCombo()->findData( fanProfileRef );
        if ( tabIdx >= 0 )
          m_fanControlTab->fanProfileCombo()->setCurrentIndex( tabIdx );
        loadedFanProfile = m_profileFanProfileCombo->itemData( idx ).toString();
      }
      else
      {
        // Referenced fan profile was not found
        fanProfileNotFound = true;
        missingFanProfile = fanProfileRef;
      }
    }

    // Load sameSpeed from profile (default true)
    if ( fanObj.contains( "sameSpeed" ) )
      m_sameFanSpeedCheckBox->setChecked( fanObj["sameSpeed"].toBool( true ) );
    else
      m_sameFanSpeedCheckBox->setChecked( true );

    if ( fanObj.contains( "autoControlWC" ) )
    {
      bool autoControl = fanObj["autoControlWC"].toBool( true );
      m_autoWaterControlCheckBox->setChecked( autoControl );
      if ( m_fanControlTab )
        m_fanControlTab->setWaterCoolerAutoControl( autoControl );
    }
    else
    {
      m_autoWaterControlCheckBox->setChecked( true );
      if ( m_fanControlTab )
        m_fanControlTab->setWaterCoolerAutoControl( true );
    }

    // Query the daemon directly for the runtime water-cooler enable state
    bool wcEnable = m_UccdClient->isWaterCoolerEnabled().value_or(
        fanObj["enableWaterCooler"].toBool( true ) );
    m_fanControlTab->setWaterCoolerEnabled( wcEnable );
    m_dashboardTab->setWaterCoolerEnabled( wcEnable );
  }

  // Load CPU settings (nested in cpu object)
  m_cpuCoreSelection->setProfile(obj["cpu"].toObject());

  if ( obj.contains( "cpu" ) && obj["cpu"].isObject() )
  {
    QJsonObject cpuObj = obj["cpu"].toObject();


    if ( cpuObj.contains( "onlineCores" ) )
      m_cpuCoresSlider->setValue(cpuObj["onlineCores"].toInt(-1)>0 ? cpuObj["onlineCores"].toInt() : m_cpuCoresSlider->maximum());
    if(cpuObj["enabledCores"].isArray()) m_cpuCoresSlider->setValue(cpuObj["enabledCores"].toArray().size());

    if ( cpuObj.contains( "governor" ) )
    {
      if ( int index = m_governorCombo->findData( cpuObj["governor"].toString() ); index >= 0 )
        m_governorCombo->setCurrentIndex( index );
      else
        m_governorCombo->setCurrentIndex( 0 ); // default to first
    }

    if ( cpuObj.contains( "energyPerformancePreference" ) && m_eppCombo )
    {
      if ( int index = m_eppCombo->findData( cpuObj["energyPerformancePreference"].toString() ); index >= 0 )
        m_eppCombo->setCurrentIndex( index );
      else
        m_eppCombo->setCurrentIndex( 0 );
    }

    // Get hardware frequency limits and set slider ranges
    if ( auto limitsJson = m_UccdClient->getCpuFrequencyLimitsJSON() )
    {
      QJsonDocument doc = QJsonDocument::fromJson( limitsJson->c_str() );
      if ( doc.isObject() )
      {
        QJsonObject limitsObj = doc.object();
        int minFreqKHz = limitsObj["min"].toInt( 400000 );   // hardware min frequency in kHz
        int maxFreqKHz = limitsObj["max"].toInt( 6000000 );  // hardware max frequency in kHz

        m_cpuMinFreqKHz = minFreqKHz;
        m_cpuMaxFreqKHz = maxFreqKHz;
        m_minFrequencySlider->setMinimum( minFreqKHz );
        m_minFrequencySlider->setMaximum( maxFreqKHz );
        m_maxFrequencySlider->setMinimum( minFreqKHz );
        m_maxFrequencySlider->setMaximum( maxFreqKHz );
      }
    }

    // Load frequency values in MHz (convert from kHz stored in profile)
    if ( cpuObj.contains( "scalingMinFrequency" ) )
    {
      int requestedKHz = cpuObj["scalingMinFrequency"].toInt( 1000000 );
      m_minFrequencySlider->setValue( requestedKHz );
    }

    if ( cpuObj.contains( "scalingMaxFrequency" ) )
    {
      int requestedKHz = cpuObj["scalingMaxFrequency"].toInt( 5000000 );
      m_maxFrequencySlider->setValue( requestedKHz );
    }

    if ( m_hwpDynamicBoostCheckBox && m_hwpDynamicBoostSupported )
      m_hwpDynamicBoostCheckBox->setChecked( cpuObj["hwpDynamicBoost"].toBool( false ) );
  }
  else
  {
    // Profile loading failed, still try to set slider limits from hardware
    if ( auto limitsJson = m_UccdClient->getCpuFrequencyLimitsJSON() )
    {
      QJsonDocument doc = QJsonDocument::fromJson( limitsJson->c_str() );
      if ( doc.isObject() )
      {
        QJsonObject limitsObj = doc.object();
        int minFreqKHz = limitsObj["min"].toInt( 400000 );
        int maxFreqKHz = limitsObj["max"].toInt( 6000000 );
        m_minFrequencySlider->setMinimum( minFreqKHz );
        m_minFrequencySlider->setMaximum( maxFreqKHz );
        m_maxFrequencySlider->setMinimum( minFreqKHz );
        m_maxFrequencySlider->setMaximum( maxFreqKHz );
      }
    }
  }

  // Load ODM Power Limits (TDP) settings (nested in odmPowerLimits object)
  // First, set slider ranges from hardware limits
  std::vector< int > hardwareLimits = m_profileManager->getHardwarePowerLimits();
  if ( hardwareLimits.size() > 0 )
  {
    m_odmPowerLimit1Slider->setMaximum( hardwareLimits[0] );
  }

  if ( hardwareLimits.size() > 1 )
  {
    m_odmPowerLimit2Slider->setMaximum( hardwareLimits[1] );
  }

  if ( hardwareLimits.size() > 2 )
  {
    m_odmPowerLimit3Slider->setMaximum( hardwareLimits[2] );
  }

  // Then, set slider values from profile

  if ( obj.contains( "odmPowerLimits" ) && obj["odmPowerLimits"].isObject() )
  {
    QJsonObject odmLimitsObj = obj["odmPowerLimits"].toObject();


    if ( odmLimitsObj.contains( "tdpValues" ) && odmLimitsObj["tdpValues"].isArray() )
    {
      QJsonArray tdpArray = odmLimitsObj["tdpValues"].toArray();

      // Load actual values from profile - these are the current settings

      if ( tdpArray.size() > 0 )
      {
        int val0 = tdpArray[0].toInt();
        m_odmPowerLimit1Slider->setValue( val0 );
      }


      if ( tdpArray.size() > 1 )
      {
        int val1 = tdpArray[1].toInt();
        m_odmPowerLimit2Slider->setValue( val1 );
      }


      if ( tdpArray.size() > 2 )
      {
        int val2 = tdpArray[2].toInt();
        m_odmPowerLimit3Slider->setValue( val2 );
      }
    }
  }
  // Load GPU Power (cTGP) setting
  refreshCtgpCapabilities();
  updateCtgpVisibility();
  int defaultPower = 0;
  int maxPower = 0;
  if ( m_ctgpSlider && hasValidCtgpRange( defaultPower, maxPower ) )
  {
    int ctgpOffset = obj.value( "nvidiaCTGPOffset" ).toInt( 0 );

    m_ctgpSlider->blockSignals( true );
    m_ctgpSlider->setMinimum( defaultPower );
    m_ctgpSlider->setMaximum( maxPower );
    m_ctgpSlider->setValue( std::clamp( defaultPower + ctgpOffset, defaultPower, maxPower ) );
    m_ctgpSlider->blockSignals( false );
    m_ctgpValueLabel->setText( QString::number( m_ctgpSlider->value() ) + " W" );
  }

  // Load Keyboard settings - check for embedded keyboard profile name
  QString loadedKeyboardProfile;
  bool keyboardProfileNotFound = false;
  QString missingKeyboardProfile;
  if ( obj.contains( "selectedKeyboardProfile" ) )
  {
    QString keyboardProfileId = obj["selectedKeyboardProfile"].toString();
    // Find by ID in combo userData
    int idx = -1;
    for ( int i = 0; i < m_profileKeyboardProfileCombo->count(); ++i )
    {
      if ( m_profileKeyboardProfileCombo->itemData( i ).toString() == keyboardProfileId )
      { idx = i; break; }
    }
    if ( idx >= 0 )
    {
      m_profileKeyboardProfileCombo->setCurrentIndex( idx );
      m_keyboardProfileCombo->setCurrentIndex( idx );
      loadedKeyboardProfile = m_profileKeyboardProfileCombo->itemData( idx ).toString();
    }
    else
    {
      // Referenced keyboard profile was not found
      keyboardProfileNotFound = true;
      missingKeyboardProfile = keyboardProfileId;
    }
  }
  // Keyboard brightness and colors are managed by the keyboard profile system
  // (via selectedKeyboardProfile), not directly from system profile data.
  // "keyboard" object in profiles is ignored to avoid overriding
  // the hardware brightness with stale saved values.

  // Load per-profile lid lighting toggles with fallback to global powerDevices.lid defaults
  bool defaultLidOffKbd = true;
  bool defaultLidOffWcLed = false;
  if ( auto pdJson = m_UccdClient->getPowerDeviceSettingsJSON() )
  {
    const auto pdDoc = QJsonDocument::fromJson( QByteArray::fromStdString( *pdJson ) );
    const auto pdLid = pdDoc.object().value( "lid" ).toObject();
    if ( pdLid.contains( "turnOffKeyboardBacklightOnLidClose" ) )
      defaultLidOffKbd = pdLid.value( "turnOffKeyboardBacklightOnLidClose" ).toBool( true );
    if ( pdLid.contains( "turnOffWaterCoolerLedOnLidClose" ) )
      defaultLidOffWcLed = pdLid.value( "turnOffWaterCoolerLedOnLidClose" ).toBool( false );
  }
  const auto lidObj = obj.value( "lid" ).toObject();
  if ( m_profileLidOffKbd )
    m_profileLidOffKbd->setChecked( lidObj.contains( "turnOffKeyboardBacklightOnLidClose" )
                                     ? lidObj.value( "turnOffKeyboardBacklightOnLidClose" ).toBool( defaultLidOffKbd )
                                     : defaultLidOffKbd );
  if ( m_profileLidOffWcLed )
    m_profileLidOffWcLed->setChecked( lidObj.contains( "turnOffWaterCoolerLedOnLidClose" )
                                       ? lidObj.value( "turnOffWaterCoolerLedOnLidClose" ).toBool( defaultLidOffWcLed )
                                       : defaultLidOffWcLed );

  // Load power state activation settings
  if ( QString settingsJson = m_profileManager->getSettingsJSON(); !settingsJson.isEmpty() )
  {
    if ( QJsonDocument settingsDoc = QJsonDocument::fromJson( settingsJson.toUtf8() );
         settingsDoc.isObject() )
    {
      QJsonObject settingsObj = settingsDoc.object();
      if ( settingsObj.contains( "stateMap" ) && settingsObj["stateMap"].isObject() )
      {
        QJsonObject stateMap = settingsObj["stateMap"].toObject();
        QString mainsProfile = stateMap["power_ac"].toString();
        QString batteryProfile = stateMap["power_bat"].toString();
        m_typeCButton->setChecked(stateMap["power_type_c"].toString() == profileId);
        QString wcProfile = stateMap["power_wc"].toString();

        m_mainsButton->setChecked( mainsProfile == profileId );
        m_batteryButton->setChecked( batteryProfile == profileId );
        m_waterCoolerButton->setChecked( wcProfile == profileId );

        // Store the loaded power state assignments
        m_loadedMainsAssignment = (mainsProfile == profileId);
        m_loadedBatteryAssignment = (batteryProfile == profileId);
        m_loadedWaterCoolerAssignment = (wcProfile == profileId);
      }
    }
  }

  // Unblock signals
  m_brightnessSlider->blockSignals( false );
  m_setBrightnessCheckBox->blockSignals( false );
  m_profileRefreshRate->blockSignals(false);
  m_profileFanProfileCombo->blockSignals( false );
  m_fanControlTab->fanProfileCombo()->blockSignals( false );
  if ( m_autoWaterControlCheckBox ) m_autoWaterControlCheckBox->blockSignals( false );

  // Set initial auto control state for water cooler
  if ( m_fanControlTab && m_autoWaterControlCheckBox )
    m_fanControlTab->setWaterCoolerAutoControl( m_autoWaterControlCheckBox->isChecked() );
  m_cpuCoresSlider->blockSignals( false );
  m_governorCombo->blockSignals( false );
  m_minFrequencySlider->blockSignals( false );
  m_maxFrequencySlider->blockSignals( false );
  m_odmPowerLimit1Slider->blockSignals( false );
  m_odmPowerLimit2Slider->blockSignals( false );
  m_odmPowerLimit3Slider->blockSignals( false );
  m_profileKeyboardProfileCombo->blockSignals( false );
  m_keyboardProfileCombo->blockSignals( false );
  if ( m_profileLidOffKbd ) m_profileLidOffKbd->blockSignals( false );
  if ( m_profileLidOffWcLed ) m_profileLidOffWcLed->blockSignals( false );
  if ( m_profileLidCloseAction ) m_profileLidCloseAction->blockSignals( false );
  m_mainsButton->blockSignals( false );
  m_typeCButton->blockSignals(false);
  m_batteryButton->blockSignals( false );
  m_waterCoolerButton->blockSignals( false );
  if ( m_descriptionEdit ) m_descriptionEdit->blockSignals( false );

  // Trigger label updates by calling the slots directly
  onBrightnessSliderChanged( m_brightnessSlider->value() );
  onCpuCoresChanged( m_cpuCoresSlider->value() );
  onMaxFrequencyChanged( m_maxFrequencySlider->value() );
  onODMPowerLimit1Changed( m_odmPowerLimit1Slider->value() );
  onODMPowerLimit2Changed( m_odmPowerLimit2Slider->value() );
  onODMPowerLimit3Changed( m_odmPowerLimit3Slider->value() );
  // Trigger fan profile change if one was loaded (loads fan curve data for display only)
  if ( !loadedFanProfile.isEmpty() )
  {
    onFanProfileChanged( loadedFanProfile );
  }

  // Load keyboard profile for display only - must NOT write to hardware.
  // onKeyboardProfileChanged() pushes states to the daemon, which would revert
  // live changes made by the tray applet while the GUI was not running.
  // Use updateKeyboardEditorFromProfile() which only updates the UI widgets.
  if ( loadedKeyboardProfile.isEmpty() && m_keyboardProfileCombo->count() > 0 )
    loadedKeyboardProfile = m_keyboardProfileCombo->currentData().toString();

  if ( !loadedKeyboardProfile.isEmpty() )
    updateKeyboardEditorFromProfile( loadedKeyboardProfile );


  // Enable/disable editing widgets based on whether profile is custom
  const bool isCustom = m_profileManager ? m_profileManager->isCustomProfile( profileId ) : false;
  updateProfileEditingWidgets( isCustom );

  // Warn user non-blockingly if any referenced subprofiles were missing
  if ( fanProfileNotFound || keyboardProfileNotFound )
  {
    QStringList missingParts;
    if ( keyboardProfileNotFound )
      missingParts << QString( "keyboard '%1'" ).arg( missingKeyboardProfile );
    if ( fanProfileNotFound )
      missingParts << QString( "fan '%1'" ).arg( missingFanProfile );
    const QString summary = QString( "Profile '%1' had missing subprofiles (%2); defaults assigned for session." )
                              .arg( m_profileCombo->currentText(), missingParts.join( QStringLiteral( ", " ) ) );
    qWarning() << summary;
    statusBar()->showMessage( summary, 5000 );
  }

}

void MainWindow::updateProfileEditingWidgets( bool isCustom )
{
  // Enable/disable editing widgets based on whether profile is custom

  // Description edit
  if ( m_descriptionEdit ) {
    m_descriptionEdit->setEnabled( isCustom );
    m_descriptionEdit->setReadOnly( !isCustom );
  }

  // Profile combo - allow renaming only for custom profiles
  if ( m_profileCombo && m_profileCombo->lineEdit() ) {
    m_profileCombo->lineEdit()->setReadOnly( !isCustom );
  }

  // Auto-activate buttons (always enabled for power state assignment)
  if ( m_mainsButton ) m_mainsButton->setEnabled( true );
  if ( m_batteryButton ) m_batteryButton->setEnabled( true );

  // Display controls
  if ( m_setBrightnessCheckBox ) m_setBrightnessCheckBox->setEnabled( isCustom );
  if ( m_profileRefreshRate ) m_profileRefreshRate->setEnabled(isCustom);
  if ( m_brightnessSlider ) m_brightnessSlider->setEnabled( isCustom );

  // Fan controls
  if ( m_profileFanProfileCombo ) m_profileFanProfileCombo->setEnabled( isCustom );
  if ( m_sameFanSpeedCheckBox ) m_sameFanSpeedCheckBox->setEnabled( isCustom );
  if ( m_autoWaterControlCheckBox ) m_autoWaterControlCheckBox->setEnabled( isCustom );

  // CPU controls
  if ( m_cpuCoresSlider ) m_cpuCoresSlider->setEnabled( isCustom && !m_cpuCoreSelection->individual() );
  m_cpuCoreSelection->setEditable(isCustom);
  if ( m_governorCombo ) m_governorCombo->setEnabled( isCustom );
  if ( m_eppCombo ) m_eppCombo->setEnabled( isCustom );
  if ( m_minFrequencySlider ) m_minFrequencySlider->setEnabled( isCustom );
  if ( m_maxFrequencySlider ) m_maxFrequencySlider->setEnabled( isCustom );
  if ( m_hwpDynamicBoostCheckBox ) m_hwpDynamicBoostCheckBox->setEnabled( isCustom );

  // Keyboard profile and per-profile lid lighting
  if ( m_profileKeyboardProfileCombo ) m_profileKeyboardProfileCombo->setEnabled( isCustom );
  if ( m_profileLidOffKbd ) m_profileLidOffKbd->setEnabled( isCustom && !readOnlyPreview );
  if ( m_profileLidOffWcLed ) m_profileLidOffWcLed->setEnabled( isCustom && !readOnlyPreview );
  if ( m_profileLidCloseAction ) m_profileLidCloseAction->setEnabled( isCustom && !readOnlyPreview );

  // ODM Power controls
  if ( m_odmPowerLimit1Slider ) m_odmPowerLimit1Slider->setEnabled( isCustom );
  if ( m_odmPowerLimit2Slider ) m_odmPowerLimit2Slider->setEnabled( isCustom );
  if ( m_odmPowerLimit3Slider ) m_odmPowerLimit3Slider->setEnabled( isCustom );

  // GPU Power (cTGP) slider
  int ctgpDefault = 0;
  int ctgpMax = 0;
  if ( m_ctgpSlider ) m_ctgpSlider->setEnabled( isCustom && hasValidCtgpRange( ctgpDefault, ctgpMax ) );
  // Charging profile
}

void MainWindow::applyWaterCoolerSupported( bool supported )
{
  m_waterCoolerSupported = supported;
  if ( auto *navigation = findChild< QListWidget * >( QStringLiteral( "navigation" ) );
       navigation && navigation->count() > 3 )
  {
    auto *item = navigation->item( 3 );
    if ( supported )
    {
      item->setFlags( item->flags() | Qt::ItemIsEnabled );
      item->setToolTip( QString() );
    }
    else
    {
      item->setFlags( item->flags() & ~Qt::ItemIsEnabled );
      item->setToolTip( QStringLiteral( "Water cooling is not supported by this device." ) );
    }
  }
  if ( m_waterCoolerStatusBarLabel )
  {
    if ( auto *row = m_waterCoolerStatusBarLabel->parentWidget() )
      row->setVisible( supported );
  }
  if ( m_dashboardTab )
    m_dashboardTab->setWaterCoolerSupported( supported );
  if ( m_fanControlTab )
    m_fanControlTab->setWaterCoolerSupported( supported );
}

void MainWindow::applyHwpDynamicBoostSupported( bool supported )
{
  m_hwpDynamicBoostSupported = supported;
  if ( m_hwpDynamicBoostLabel )
    m_hwpDynamicBoostLabel->setVisible( supported );
  if ( m_hwpDynamicBoostCheckBox )
    m_hwpDynamicBoostCheckBox->setVisible( supported );
}

void MainWindow::fetchStartupCapabilities()
{
  m_cTGPAdjustmentSupported = false;
  m_nvidiaPowerCtrlAvailable = false;
  m_gpuDefaultPowerLimit = 0;
  m_gpuMaxPowerLimit = 0;
  if ( !m_UccdClient || !m_UccdClient->isConnected() )
  {
    applyWaterCoolerSupported( false );
    applyHwpDynamicBoostSupported( false );
    updateCtgpVisibility();
    return;
  }

  QString destination = QStringLiteral( "com.uniwill.uccd" );
  if ( readOnlyPreview )
  {
    auto *busIface = QDBusConnection::systemBus().interface();
    destination = busIface ? busIface->serviceOwner( destination ).value() : QString();
    if ( destination.isEmpty() )
      return;
  }

  auto asyncCapabilityCall = [this, destination]( const QString &method, auto onDone ) {
    auto msg = QDBusMessage::createMethodCall(
      destination,
      QStringLiteral( "/com/uniwill/uccd" ),
      QStringLiteral( "com.uniwill.uccd" ),
      method );
    msg.setAutoStartService( false );
    auto *watcher = new QDBusPendingCallWatcher(
      QDBusConnection::systemBus().asyncCall( msg, 1500 ),
      this );
    connect( watcher, &QDBusPendingCallWatcher::finished, this,
      [this, onDone = std::move( onDone )]( QDBusPendingCallWatcher *done ) {
        const QDBusMessage reply = done->reply();
        done->deleteLater();
        if ( m_closing )
          return;
        onDone( reply );
      } );
  };

  asyncCapabilityCall( QStringLiteral( "GetWaterCoolerSupported" ), [this]( const QDBusMessage &reply ) {
    if ( reply.type() != QDBusMessage::ErrorMessage && !reply.arguments().isEmpty() )
      applyWaterCoolerSupported( reply.arguments().front().toBool() );
  } );
  asyncCapabilityCall( QStringLiteral( "GetHwpDynamicBoostSupported" ), [this]( const QDBusMessage &reply ) {
    if ( reply.type() != QDBusMessage::ErrorMessage && !reply.arguments().isEmpty() )
      applyHwpDynamicBoostSupported( reply.arguments().front().toBool() );
  } );
  asyncCapabilityCall( QStringLiteral( "GetCTGPAdjustmentSupported" ), [this]( const QDBusMessage &reply ) {
    if ( reply.type() != QDBusMessage::ErrorMessage && !reply.arguments().isEmpty() )
    {
      m_cTGPAdjustmentSupported = reply.arguments().front().toBool();
      updateCtgpVisibility();
    }
  } );
  asyncCapabilityCall( QStringLiteral( "GetNVIDIAPowerCTRLAvailable" ), [this]( const QDBusMessage &reply ) {
    if ( reply.type() != QDBusMessage::ErrorMessage && !reply.arguments().isEmpty() )
    {
      m_nvidiaPowerCtrlAvailable = reply.arguments().front().toBool();
      updateCtgpVisibility();
    }
  } );
  asyncCapabilityCall( QStringLiteral( "GetNVIDIAPowerCTRLDefaultPowerLimit" ), [this]( const QDBusMessage &reply ) {
    if ( reply.type() != QDBusMessage::ErrorMessage && !reply.arguments().isEmpty() )
    {
      m_gpuDefaultPowerLimit = reply.arguments().front().toInt();
      updateCtgpVisibility();
    }
  } );
  asyncCapabilityCall( QStringLiteral( "GetNVIDIAPowerCTRLMaxPowerLimit" ), [this]( const QDBusMessage &reply ) {
    if ( reply.type() != QDBusMessage::ErrorMessage && !reply.arguments().isEmpty() )
    {
      m_gpuMaxPowerLimit = reply.arguments().front().toInt();
      updateCtgpVisibility();
    }
  } );
  asyncCapabilityCall( QStringLiteral( "GetDisplayModesJSON" ), [this]( const QDBusMessage &reply ) {
    if ( reply.type() == QDBusMessage::ErrorMessage || reply.arguments().isEmpty() )
      return;
    const QJsonDocument doc = QJsonDocument::fromJson( reply.arguments().front().toString().toUtf8() );
    if ( !doc.isObject() )
      return;
    const QJsonObject root = doc.object();
    const QJsonObject activeMode = root.value( QStringLiteral( "activeMode" ) ).toObject();
    const int activeX = activeMode.value( QStringLiteral( "xResolution" ) ).toInt( 0 );
    const int activeY = activeMode.value( QStringLiteral( "yResolution" ) ).toInt( 0 );
    const QJsonArray displayModes = root.value( QStringLiteral( "displayModes" ) ).toArray();
    std::set< int > uniqueRates;
    auto collectRates = [&uniqueRates]( const QJsonArray &arr ) {
      for ( const auto &v : arr )
      {
        const int hz = qRound( v.toDouble() );
        if ( hz > 0 )
          uniqueRates.insert( hz );
      }
    };
    collectRates( activeMode.value( QStringLiteral( "refreshRates" ) ).toArray() );
    bool matchedActiveRes = false;
    if ( activeX > 0 && activeY > 0 )
    {
      for ( const auto &modeVal : displayModes )
      {
        const QJsonObject modeObj = modeVal.toObject();
        if ( modeObj.value( QStringLiteral( "xResolution" ) ).toInt( 0 ) == activeX &&
             modeObj.value( QStringLiteral( "yResolution" ) ).toInt( 0 ) == activeY )
        {
          collectRates( modeObj.value( QStringLiteral( "refreshRates" ) ).toArray() );
          matchedActiveRes = true;
        }
      }
    }
    if ( !matchedActiveRes )
    {
      for ( const auto &modeVal : displayModes )
        collectRates( modeVal.toObject().value( QStringLiteral( "refreshRates" ) ).toArray() );
    }
    if ( !uniqueRates.empty() )
    {
      m_supportedRefreshRates.clear();
      for ( int hz : uniqueRates )
        m_supportedRefreshRates.push_back( hz );
      const int currentHz = m_profileRefreshRate ? m_profileRefreshRate->currentData().toInt() : 0;
      rebuildRefreshRateCombo( currentHz );
    }
  } );
}

void MainWindow::rebuildRefreshRateCombo( int selectedHz )
{
  if ( !m_profileRefreshRate )
    return;
  const QSignalBlocker blocked( m_profileRefreshRate );
  m_profileRefreshRate->clear();
  m_profileRefreshRate->addItem( QStringLiteral( "Do not change" ), 0 );
  const QVector< int > rates = m_supportedRefreshRates.isEmpty()
    ? QVector< int >{ 60, 120, 144, 165, 240 }
    : m_supportedRefreshRates;
  for ( int hz : rates )
    m_profileRefreshRate->addItem( QStringLiteral( "%1 Hz" ).arg( hz ), hz );
  if ( selectedHz > 0 && !rates.contains( selectedHz ) )
    m_profileRefreshRate->addItem( QStringLiteral( "%1 Hz (unsupported)" ).arg( selectedHz ), selectedHz );
  const int idx = m_profileRefreshRate->findData( selectedHz );
  m_profileRefreshRate->setCurrentIndex( qMax( 0, idx ) );
}

void MainWindow::refreshCtgpCapabilities()
{
  m_cTGPAdjustmentSupported = false;
  m_nvidiaPowerCtrlAvailable = false;
  m_gpuDefaultPowerLimit = 0;
  m_gpuMaxPowerLimit = 0;
  if ( !m_UccdClient )
    return;

  if ( auto ctgp = m_UccdClient->getCTGPAdjustmentSupported() )
    m_cTGPAdjustmentSupported = *ctgp;
  if ( auto avail = m_UccdClient->getNVIDIAPowerCTRLAvailable() )
    m_nvidiaPowerCtrlAvailable = *avail;
  if ( auto gpuDefault = m_UccdClient->getNVIDIAPowerCTRLDefaultPowerLimit() )
    m_gpuDefaultPowerLimit = *gpuDefault;
  if ( auto gpuMax = m_UccdClient->getNVIDIAPowerCTRLMaxPowerLimit() )
    m_gpuMaxPowerLimit = *gpuMax;
}

bool MainWindow::hasValidCtgpRange( int &defaultPower, int &maxPower ) const
{
  if ( !m_cTGPAdjustmentSupported || !m_nvidiaPowerCtrlAvailable )
    return false;
  if ( m_gpuDefaultPowerLimit <= 0 || m_gpuMaxPowerLimit < m_gpuDefaultPowerLimit )
    return false;
  defaultPower = m_gpuDefaultPowerLimit;
  maxPower = m_gpuMaxPowerLimit;
  return true;
}

void MainWindow::updateCtgpVisibility()
{
  int defaultPower = 0;
  int maxPower = 0;
  const bool visible = hasValidCtgpRange( defaultPower, maxPower );

  if ( m_ctgpHeader )
    m_ctgpHeader->setVisible( visible );
  if ( m_ctgpLabel )
    m_ctgpLabel->setVisible( visible );
  if ( m_ctgpWidget )
    m_ctgpWidget->setVisible( visible );
}

void MainWindow::markChanged()
{
  m_profileChanged = true;
  updateButtonStates();
}

void MainWindow::updateButtonStates( void)
{
  // Update profile page buttons if available
  if ( profileTopWidgetsAvailable() )
  {
    m_removeProfileButton->setEnabled( m_profileManager->isCustomProfile( m_profileCombo->currentData().toString() ) );
  }

  // Delegate fan profile button states to FanControlTab
  if ( m_fanControlTab )
    m_fanControlTab->updateButtonStates( m_UccdClient->isConnected() );
}

QString MainWindow::buildProfileJSON() const
{
  QString profileId   = m_profileCombo->currentData().toString();
  QString profileName = m_profileCombo->currentText();

  QJsonObject profileObj;
  profileObj["id"]          = profileId;
  profileObj["name"]        = profileName;
  profileObj["description"] = m_descriptionEdit->toPlainText();

  // Brightness
  QJsonObject displayObj = QJsonDocument::fromJson(m_profileManager->getProfileDetails(profileId).toUtf8()).object()["display"].toObject();
  const bool useBrightness = m_setBrightnessCheckBox->isChecked();
  displayObj["useBrightness"] = useBrightness;
  if ( useBrightness )
    displayObj["brightness"] = m_brightnessSlider->value();
  const int refreshHz = m_profileRefreshRate->currentData().toInt();
  displayObj["useRefRate"] = refreshHz > 0;
  displayObj["refreshRate"] = refreshHz > 0 ? refreshHz : -1;
  if ( m_profileLidCloseAction )
  {
    const QString lidAction = m_profileLidCloseAction->currentData().toString();
    if ( lidAction.isEmpty() )
      displayObj.remove( QStringLiteral( "lidCloseAction" ) );
    else
      displayObj["lidCloseAction"] = lidAction;
  }
  profileObj["display"] = displayObj;

  // Fan - embed complete fan profile tables
  QJsonObject fanObj;
  QString fanProfileId  = m_profileFanProfileCombo->currentData().toString();
  QString fanProfileJSON = m_profileManager->getFanProfile( fanProfileId );
  if ( !fanProfileJSON.isEmpty() && fanProfileJSON != "{}" )
  {
    QJsonDocument fanDoc = QJsonDocument::fromJson( fanProfileJSON.toUtf8() );
    if ( fanDoc.isObject() )
    {
      QJsonObject fp = fanDoc.object();
      if ( fp.contains( "tableCPU" ) )           fanObj["tableCPU"]           = fp["tableCPU"];
      if ( fp.contains( "tableGPU" ) )           fanObj["tableGPU"]           = fp["tableGPU"];
      if ( fp.contains( "tablePump" ) )          fanObj["tablePump"]          = fp["tablePump"];
      if ( fp.contains( "tableWaterCoolerFan" ) ) fanObj["tableWaterCoolerFan"] = fp["tableWaterCoolerFan"];
    }
  }
  // Carry manual settings from the running profile, or the selected saved profile.
  QJsonObject sourceFan;
  if (m_currentLoadedProfile == m_profileManager->activeProfileId()) {
    if (auto runtime = m_UccdClient->getActiveProfileJSON())
      sourceFan = QJsonDocument::fromJson(QByteArray::fromStdString(*runtime)).object()["fan"].toObject();
  } else {
    sourceFan = QJsonDocument::fromJson(m_profileManager->getProfileDetails(m_currentLoadedProfile).toUtf8()).object()["fan"].toObject();
  }
  fanObj["manualFanSpeed"] = sourceFan.value("manualFanSpeed").toInt(-1);
  fanObj["manualPumpVoltage"] = sourceFan.value("manualPumpVoltage").toInt(-1);
  fanObj["fanProfile"]       = fanProfileId;
  fanObj["sameSpeed"]        = m_sameFanSpeedCheckBox   ? m_sameFanSpeedCheckBox->isChecked()   : true;
  fanObj["autoControlWC"]    = m_autoWaterControlCheckBox ? m_autoWaterControlCheckBox->isChecked() : true;
  fanObj["enableWaterCooler"] = m_fanControlTab          ? m_fanControlTab->isWaterCoolerEnabled() : true;
  profileObj["fan"] = fanObj;

  // CPU
  QJsonObject cpuObj;
  cpuObj["onlineCores"]                = m_cpuCoresSlider->value();
  m_cpuCoreSelection->writeProfile(cpuObj);
  cpuObj["governor"]                   = m_governorCombo->currentData().toString();
  cpuObj["energyPerformancePreference"] = m_eppCombo ? m_eppCombo->currentData().toString() : QString();
  const int cpuMinFreqKHz = std::min( m_cpuMinFreqKHz, m_cpuMaxFreqKHz );
  const int cpuMaxFreqKHz = std::max( m_cpuMinFreqKHz, m_cpuMaxFreqKHz );
  cpuObj["scalingMinFrequency"]         = std::clamp( m_minFrequencySlider->value(), cpuMinFreqKHz, cpuMaxFreqKHz );
  cpuObj["scalingMaxFrequency"]         = std::clamp( m_maxFrequencySlider->value(), cpuMinFreqKHz, cpuMaxFreqKHz );
  if ( m_hwpDynamicBoostCheckBox && m_hwpDynamicBoostSupported )
    cpuObj["hwpDynamicBoost"]           = m_hwpDynamicBoostCheckBox->isChecked();
  profileObj["cpu"] = cpuObj;

  // ODM Power Limits (TDP)
  QJsonObject odmObj;
  QJsonArray tdpArray;
  tdpArray.append( m_odmPowerLimit1Slider->value() );
  tdpArray.append( m_odmPowerLimit2Slider->value() );
  tdpArray.append( m_odmPowerLimit3Slider->value() );
  odmObj["tdpValues"] = tdpArray;
  profileObj["odmPowerLimits"] = odmObj;

  // GPU Power (cTGP) - embed cTGP offset directly in the profile
  int ctgpDefaultPower = 0;
  int ctgpMaxPower = 0;
  if ( m_ctgpSlider && hasValidCtgpRange( ctgpDefaultPower, ctgpMaxPower ) )
  {
    int sliderValue = std::clamp( m_ctgpSlider->value(), ctgpDefaultPower, ctgpMaxPower );
    int ctgpOffset = sliderValue - ctgpDefaultPower;
    profileObj["nvidiaCTGPOffset"] = ctgpOffset;
  }
  else
  {
    const QJsonObject existingObj =
      QJsonDocument::fromJson( m_profileManager->getProfileDetails( profileId ).toUtf8() ).object();
    if ( existingObj.contains( "nvidiaCTGPOffset" ) )
      profileObj["nvidiaCTGPOffset"] = existingObj.value( "nvidiaCTGPOffset" );
  }

  // Keyboard - only store the profile reference, not the full state data
  // Full per-key states live in customKeyboardProfiles managed by uccd.
  QString keyboardProfileId  = m_profileKeyboardProfileCombo->currentData().toString();
  QJsonObject keyboardObj;
  if ( m_keyboardBrightnessSlider )
    keyboardObj["brightness"] = m_keyboardBrightnessSlider->value();
  profileObj["keyboard"]               = keyboardObj;
  profileObj["selectedKeyboardProfile"] = keyboardProfileId;

  if ( m_profileLidOffKbd || m_profileLidOffWcLed )
  {
    QJsonObject lidObj;
    if ( m_profileLidOffKbd )
      lidObj["turnOffKeyboardBacklightOnLidClose"] = m_profileLidOffKbd->isChecked();
    if ( m_profileLidOffWcLed )
      lidObj["turnOffWaterCoolerLedOnLidClose"] = m_profileLidOffWcLed->isChecked();
    profileObj["lid"] = lidObj;
  }

  return QString::fromUtf8( QJsonDocument( profileObj ).toJson() );
}

void MainWindow::onApplyClicked()
{
  if ( m_selectedProfileIndex < 0 )
  {
    statusBar()->showMessage( "No profile selected" );
    return;
  }
  if ( m_pendingApplyWatcher )
    return;

  const QString appliedName = m_profileCombo->currentText();
  const QString profileJSON = buildProfileJSON();
  if ( m_applyButton )
    m_applyButton->setEnabled( false );
  statusBar()->showMessage( QStringLiteral( "Applying profile: " ) + appliedName + QStringLiteral( "…" ) );

  m_pendingApplyWatcher = m_profileManager->getClient()->applyProfileAsync(
    profileJSON.toStdString(),
    this,
    [this, appliedName]( bool ok, const QString &err ) {
      m_pendingApplyWatcher.clear();
      if ( m_closing )
        return;
      if ( m_applyButton )
        m_applyButton->setEnabled( true );
      if ( !ok )
      {
        if ( m_profileManager )
        {
          const QString activeId = m_profileManager->activeProfileId();
          if ( !activeId.isEmpty() )
            loadProfileDetails( activeId );
        }
        const QString msg = err.isEmpty()
          ? QStringLiteral( "Failed to apply profile: %1" ).arg( appliedName )
          : QStringLiteral( "Failed to apply profile %1: %2" ).arg( appliedName, err );
        statusBar()->showMessage( msg, 5000 );
        return;
      }

      // Re-send the current water cooler enable state so that
      // the profile apply doesn't override the user's checkbox state.
      if ( m_fanControlTab )
        m_fanControlTab->sendWaterCoolerEnable( m_fanControlTab->isWaterCoolerEnabled() );

      statusBar()->showMessage( QStringLiteral( "Profile applied: " ) + appliedName );
    } );
}

void MainWindow::onSaveClicked()
{
  QString profileName = m_profileCombo->currentText();
  QString profileId = m_profileCombo->currentData().toString();
  const bool isCustom = m_profileManager->isCustomProfile( profileId );

  m_saveInProgress = true;
  if ( isCustom )
  {
    if ( !m_profileManager->saveProfile( buildProfileJSON() ) )
    {
      m_saveInProgress = false;
      updateButtonStates();
      return;
    }
  }

  // Update stateMap for both checked power-state buttons and unchecked buttons previously mapped to this profile.
  std::map< QString, QString > stateMapUpdates;
  const auto syncStateButton = [&]( const QString &stateKey, const QPushButton *btn ) {
    if ( !btn )
      return;
    if ( btn->isChecked() )
      stateMapUpdates[stateKey] = profileId;
    else if ( m_profileManager->resolveStateMapToProfileId( stateKey ) == profileId )
      stateMapUpdates[stateKey] = QString();
  };
  syncStateButton( QStringLiteral( "power_ac" ), m_mainsButton );
  syncStateButton( QStringLiteral( "power_type_c" ), m_typeCButton );
  syncStateButton( QStringLiteral( "power_bat" ), m_batteryButton );
  syncStateButton( QStringLiteral( "power_wc" ), m_waterCoolerButton );

  if ( !stateMapUpdates.empty() && !m_profileManager->setBatchStateMap( stateMapUpdates ) )
  {
    m_saveInProgress = false;
    updateButtonStates();
    return;
  }

  m_saveInProgress = false;
  m_profileChanged = false;
  statusBar()->showMessage( ( isCustom || !stateMapUpdates.empty() )
                              ? QStringLiteral( "Profile saved successfully" )
                              : QStringLiteral( "Built-in profile updated" ) );
  updateButtonStates();
  if ( m_saveButton )
    m_saveButton->setEnabled( true );
}

void MainWindow::onSaveFanProfilesClicked()
{
  // Save fan profiles via DBus
  saveFanPoints();
  statusBar()->showMessage( "Fan profiles saved" );
}

void MainWindow::onAddProfileClicked()
{
  // Generate a unique profile name
  QString baseName = "New Profile";
  QString profileName;
  int counter = 1;

  do {
    profileName = QString("%1 %2").arg(baseName).arg(counter);
    counter++;
  } while (m_profileManager->allProfiles().contains(profileName));

  // Create profile from default
  QString profileJson = m_profileManager->createProfileFromDefault(profileName);
  if (!profileJson.isEmpty()) {
    statusBar()->showMessage( QString("Profile '%1' created successfully").arg(profileName) );

    // Switch to the newly created profile
    if (int newIndex = m_profileCombo->findText(profileName); newIndex != -1) {
      m_profileCombo->setCurrentIndex(newIndex);
    }
  }
  else
    QMessageBox::warning(this, "Error", "Failed to create new profile.");
}
void MainWindow::onCopyProfileClicked()
{
  QString current = m_profileCombo->currentText();

  // Strip " [Built-in]" suffix if present (built-in profiles shouldn't keep this marker when copied)
  if ( current.endsWith( " [Built-in]" ) )
    current = current.left( current.size() - 11 ); // 11 is length of " [Built-in]"

  // Allow copying any profile (built-in or custom)
  QString profileId = m_profileCombo->currentData().toString();
  QString json = m_profileManager->getProfileDetails(profileId);
  if (json.isEmpty()) {
    QMessageBox::warning(this, "Error", "Failed to get profile data.");
    return;
  }

  QJsonDocument doc = QJsonDocument::fromJson(json.toUtf8());
  if (!doc.isObject()) {
    QMessageBox::warning(this, "Error", "Invalid profile data.");
    return;
  }

  QJsonObject obj = doc.object();

  // Generate a new unique ID for the copied profile
  obj["id"] = QUuid::createUuid().toString(QUuid::WithoutBraces);

  // Generate new name: "New {name}" with optional incrementing number
  QString baseName = QString("New %1").arg(current);
  QString newName = baseName;
  int counter = 1;
  while (m_profileManager->allProfiles().contains(newName)) {
    newName = QString("%1 %2").arg(baseName).arg(counter);
    counter++;
  }

  // Set new name
  obj["name"] = newName;

  // Save
  QString newJson = QJsonDocument(obj).toJson(QJsonDocument::Compact);
  m_profileManager->saveProfile(newJson);

  // Switch
  if (int newIndex = m_profileCombo->findText(newName); newIndex != -1) {
    m_profileCombo->setCurrentIndex(newIndex);
  }

  statusBar()->showMessage( QString("Profile '%1' copied to '%2'").arg(current).arg(newName) );
}

void MainWindow::onRemoveProfileClicked()
{
  QString currentProfile = m_profileCombo->currentText();
  QString currentProfileId = m_profileCombo->currentData().toString();

  // Check if it's a built-in profile
  if (!m_profileManager->isCustomProfile(currentProfileId)) {
    QMessageBox::information(this, "Cannot Remove",
                            "Built-in profiles cannot be removed.");
    return;
  }

  // Confirm deletion
  QMessageBox::StandardButton reply = QMessageBox::question(
    this, "Remove Profile",
    QString("Are you sure you want to remove the profile '%1'?").arg(currentProfile),
    QMessageBox::Yes | QMessageBox::No
  );

  if (reply == QMessageBox::Yes) {
    m_profileManager->deleteProfile(currentProfileId);
    statusBar()->showMessage( QString("Profile '%1' removed").arg(currentProfile) );
  }
}

void MainWindow::onProfileComboRenamed()
{
  if ( !m_profileCombo || !m_profileCombo->lineEdit() ) return;

  int idx = m_profileCombo->currentIndex();
  if ( idx < 0 ) return;

  QString oldName = m_profileCombo->itemText( idx );
  QString newName = m_profileCombo->currentText().trimmed();

  if ( newName.isEmpty() || newName == oldName ) {
    // Revert if empty or unchanged
    m_profileCombo->setEditText( oldName );
    return;
  }

  if ( !m_profileManager->isCustomProfile( m_profileCombo->itemData( idx ).toString() ) ) {
    // Cannot rename built-in profiles
    m_profileCombo->setEditText( oldName );
    return;
  }

  // Get profile data and update the name
  QString profileId = m_profileCombo->itemData( idx ).toString();
  QString json = m_profileManager->getProfileDetails( profileId );
  if ( json.isEmpty() ) {
    m_profileCombo->setEditText( oldName );
    return;
  }

  QJsonDocument doc = QJsonDocument::fromJson( json.toUtf8() );
  if ( !doc.isObject() ) {
    m_profileCombo->setEditText( oldName );
    return;
  }

  QJsonObject obj = doc.object();
  obj["name"] = newName;

  // Save with new name (ProfileManager matches by ID and updates the name)
  QJsonDocument out( obj );
  m_profileManager->saveProfile( QString::fromUtf8( out.toJson( QJsonDocument::Compact ) ) );

  // Select the renamed profile after the combo gets repopulated
  int newIdx = m_profileCombo->findText( newName );
  if ( newIdx != -1 )
    m_profileCombo->setCurrentIndex( newIdx );

  statusBar()->showMessage( QString("Profile renamed from '%1' to '%2'").arg( oldName, newName ) );
}

void MainWindow::onKeyboardProfileComboRenamed()
{
  if ( !m_keyboardProfileCombo || !m_keyboardProfileCombo->lineEdit() ) return;

  int idx = m_keyboardProfileCombo->currentIndex();
  if ( idx < 0 ) return;

  QString oldName = m_keyboardProfileCombo->itemText( idx );
  QString newName = m_keyboardProfileCombo->currentText().trimmed();

  if ( newName.isEmpty() || newName == oldName ) {
    m_keyboardProfileCombo->setEditText( oldName );
    return;
  }

  // "Default" is built-in
  QString keyboardProfileId = m_keyboardProfileCombo->itemData( idx ).toString();
  if ( keyboardProfileId.isEmpty() || !m_profileManager->customKeyboardProfiles().contains( oldName ) ) {
    m_keyboardProfileCombo->setEditText( oldName );
    return;
  }

  if ( m_profileManager->renameKeyboardProfile( keyboardProfileId, newName ) ) {
    m_keyboardProfileCombo->setItemText( idx, newName );
    statusBar()->showMessage( QString("Keyboard profile renamed from '%1' to '%2'").arg( oldName, newName ) );
    updateKeyboardProfileButtonStates();
  } else {
    m_keyboardProfileCombo->setEditText( oldName );
    statusBar()->showMessage( "Failed to rename keyboard profile", 3000 );
  }
}

void MainWindow::onRemoveFanProfileClicked()
{
  QString currentProfile = m_fanControlTab->fanProfileCombo()->currentText();
  QString fanProfileId = m_fanControlTab->fanProfileCombo()->currentData().toString();

  // Check if it's a built-in profile
  if ( m_fanControlTab->builtinFanProfiles().contains( currentProfile ) ) {
    QMessageBox::information(this, "Cannot Remove",
                            "Built-in fan profiles cannot be removed.");
    return;
  }

  // Check if any system profiles reference this fan profile
  QStringList referencingProfiles;
  auto checkProfiles = [&]( const QJsonArray &profiles ) {
    for ( const auto &p : profiles )
    {
      if ( !p.isObject() ) continue;
      QJsonObject obj = p.toObject();
      QString name = obj["name"].toString();
      if ( obj.contains( "fan" ) && obj["fan"].isObject() )
      {
        QString ref = obj["fan"].toObject()["fanProfile"].toString();
        if ( ref == fanProfileId || ref == currentProfile )
          referencingProfiles << name;
      }
    }
  };
  checkProfiles( m_profileManager->defaultProfilesData() );
  checkProfiles( m_profileManager->customProfilesData() );

  // Build confirmation message
  QString confirmMessage;
  if ( !referencingProfiles.isEmpty() )
  {
    confirmMessage = QString( "The fan profile '%1' is referenced by the following system profiles:\n\n" ).arg( currentProfile );
    for ( const QString &name : referencingProfiles )
      confirmMessage += QString( "  - %1\n" ).arg( name );
    confirmMessage += "\nAre you sure you want to remove this fan profile?";
  }
  else
  {
    confirmMessage = QString( "Are you sure you want to remove the fan profile '%1'?" ).arg( currentProfile );
  }

  // Confirm deletion
  QMessageBox::StandardButton reply = QMessageBox::question(
    this, "Remove Fan Profile",
    confirmMessage,
    QMessageBox::Yes | QMessageBox::No
  );

  if (reply == QMessageBox::Yes) {
    // Remove from persistent storage and UI
    if ( m_profileManager->deleteFanProfile( fanProfileId ) ) {
      // Remove from both fan profile lists
      int idx = m_fanControlTab->fanProfileCombo()->currentIndex();
      if ( idx >= 0 ) m_fanControlTab->fanProfileCombo()->removeItem( idx );
      if ( m_profileFanProfileCombo ) {
        int idx2 = m_profileFanProfileCombo->findText( currentProfile );
        if ( idx2 != -1 ) m_profileFanProfileCombo->removeItem( idx2 );
      }
      statusBar()->showMessage( QString("Fan profile '%1' removed").arg(currentProfile) );
    } else {
      QMessageBox::warning(this, "Remove Failed", "Failed to remove custom fan profile.");
    }
  }
}

void MainWindow::onFanProfileChanged(const QString& fanProfileId)
{
  QString json = m_profileManager->getFanProfile(fanProfileId);
  QJsonDocument doc = QJsonDocument::fromJson(json.toUtf8());
  if (doc.isObject()) {
    QJsonObject obj = doc.object();

    // Load CPU points
    if (obj.contains("tableCPU")) {
      QJsonArray arr = obj["tableCPU"].toArray();
      QVector<FanCurveEditorWidget::Point> cpuPoints;
      for (const QJsonValue &v : arr) {
        QJsonObject o = v.toObject();
        double temp = o["temp"].toDouble();
        double speed = o["speed"].toDouble();
        cpuPoints.append({temp, speed});
      }
      if (m_fanControlTab->cpuEditor() && !cpuPoints.isEmpty()) {
        m_fanControlTab->cpuEditor()->setPoints(cpuPoints);
      }
    }

    // Load GPU points
    if (obj.contains("tableGPU")) {
      QJsonArray arr = obj["tableGPU"].toArray();
      QVector<FanCurveEditorWidget::Point> gpuPoints;
      for (const QJsonValue &v : arr) {
        QJsonObject o = v.toObject();
        double temp = o["temp"].toDouble();
        double speed = o["speed"].toDouble();
        gpuPoints.append({temp, speed});
      }
      if (m_fanControlTab->gpuEditor() && !gpuPoints.isEmpty()) {
        m_fanControlTab->gpuEditor()->setPoints(gpuPoints);
      }
    }

    // Load water cooler fan points
    if (obj.contains("tableWaterCoolerFan")) {
      QJsonArray arr = obj["tableWaterCoolerFan"].toArray();
      QVector<FanCurveEditorWidget::Point> wcFanPoints;
      for (const QJsonValue &v : arr) {
        QJsonObject o = v.toObject();
        double temp = o["temp"].toDouble();
        double speed = o["speed"].toDouble();
        wcFanPoints.append({temp, speed});
      }
      if (m_fanControlTab->wcFanEditor() && !wcFanPoints.isEmpty()) {
        m_fanControlTab->wcFanEditor()->setPoints(wcFanPoints);
      }
    }

    // Load pump threshold points
    if (obj.contains("tablePump")) {
      QJsonArray arr = obj["tablePump"].toArray();
      QVector<PumpCurveEditorWidget::Point> pumpPoints;
      for (const QJsonValue &v : arr) {
        QJsonObject o = v.toObject();
        double temp = o["temp"].toDouble();
        int speed = o["speed"].toInt();
        pumpPoints.append({temp, speed});
      }
      if (m_fanControlTab->pumpEditor() && !pumpPoints.isEmpty()) {
        m_fanControlTab->pumpEditor()->setPoints(pumpPoints);
      }
    }
  }

  // Update the current fan profile ID
  m_currentFanProfile = fanProfileId;

  // Synchronize profile tab fan profile combo with fan tab selection (match by ID userData)
  m_profileFanProfileCombo->blockSignals(true);
  int idx = -1;
  for ( int i = 0; i < m_profileFanProfileCombo->count(); ++i )
  {
    if ( m_profileFanProfileCombo->itemData( i ).toString() == fanProfileId )
    {
      idx = i;
      break;
    }
  }
  if (idx >= 0) {
    m_profileFanProfileCombo->setCurrentIndex(idx);
  }
  m_profileFanProfileCombo->blockSignals(false);

  // Set editors editable only for custom profiles (those not in built-ins)
  bool isEditable = !m_fanControlTab->builtinFanProfiles().contains( fanProfileId );
  m_fanControlTab->setEditorsEditable( isEditable );

  // Update button states
  updateButtonStates();
}

void MainWindow::onCpuFanPointsChanged(const QVector<FanCurveEditorWidget::Point>& points)
{
  m_cpuFanPoints.clear();

  for (const auto& p : points) {
    m_cpuFanPoints.append({static_cast<int>(p.temp), static_cast<int>(p.duty)});
  }
}

void MainWindow::reloadFanProfiles()
{
  // Delegate to FanControlTab which owns the fan-control combo and builtin list
  m_fanControlTab->reloadFanProfiles();

  // Populate the Profiles page combo with ALL built-in + custom fan profiles regardless of Watercool filter
  if ( m_profileFanProfileCombo )
  {
    const QString prevId = m_profileFanProfileCombo->currentData().toString();
    m_profileFanProfileCombo->blockSignals(true);
    m_profileFanProfileCombo->clear();
    QStringList seenIds;
    for ( const auto &v : m_profileManager->builtinFanProfilesData() )
    {
      if ( !v.isObject() ) continue;
      QJsonObject o = v.toObject();
      QString id = o["id"].toString();
      m_profileFanProfileCombo->addItem( o["name"].toString(), id );
      seenIds.append( id );
    }
    for ( const auto &v : m_profileManager->customFanProfilesData() )
    {
      if ( !v.isObject() ) continue;
      QJsonObject o = v.toObject();
      QString id = o["id"].toString();
      if ( !seenIds.contains( id ) )
        m_profileFanProfileCombo->addItem( o["name"].toString(), id );
    }
    if ( !prevId.isEmpty() )
    {
      const int idx = m_profileFanProfileCombo->findData( prevId );
      if ( idx >= 0 )
        m_profileFanProfileCombo->setCurrentIndex( idx );
    }
    m_profileFanProfileCombo->blockSignals(false);
  }

  // Trigger change handler to update editors/buttons
  if ( m_fanControlTab->fanProfileCombo() && m_fanControlTab->fanProfileCombo()->count() > 0 )
    onFanProfileChanged( m_fanControlTab->fanProfileCombo()->currentData().toString() );
  else
    updateButtonStates();
}



void MainWindow::onUccdConnectionChanged( bool connected )
{
  qDebug() << "[MainWindow] uccd connection changed:" << connected;

  if ( !connected )
  {
    // Display disconnected status unless waiting on post-sleep reconnect
    if ( m_connectionLabel && !m_resumingFromSleep )
      m_connectionLabel->setText(
        QStringLiteral("Profile: —") );
  }
  else
  {
    // When connected, display the active profile name
    updateConnectionStatusLabel();
  }

  if ( !connected )
    return;

  // Re-query capabilities that are only set at startup
  fetchStartupCapabilities();

  updateCtgpVisibility();

  // Repopulate driver-reported option lists
  reloadFanProfiles();
  populateGovernorCombo();
  populateEppCombo();

  // Refresh button/widget enabled states
  updateButtonStates();
}

void MainWindow::onGpuFanPointsChanged(const QVector<FanCurveEditorWidget::Point>& points)
{
  m_gpuFanPoints.clear();
  for (const auto& p : points) {
    m_gpuFanPoints.append({static_cast<int>(p.temp), static_cast<int>(p.duty)});
  }
}

void MainWindow::onWaterCoolerFanPointsChanged(const QVector<FanCurveEditorWidget::Point>& points)
{
  m_waterCoolerFanPoints.clear();
  for (const auto& p : points) {
    m_waterCoolerFanPoints.append({static_cast<int>(p.temp), static_cast<int>(p.duty)});
  }
}

void MainWindow::onPumpPointsChanged(const QVector<PumpCurveEditorWidget::Point>& /*points*/)
{
  // Pump points changed - mark the fan profile as modified so it can be saved
  if ( m_fanControlTab )
  {
    m_fanControlTab->saveButton()->setEnabled( true );
    m_fanControlTab->applyButton()->setEnabled( true );
  }
}

void MainWindow::onCopyFanProfileClicked()
{
  QString currentProfileId = m_fanControlTab->fanProfileCombo()->currentData().toString();

  if ( currentProfileId.isEmpty() )
    return;

  // Get the current profile name and strip " [Built-in]" suffix if present
  QString currentName = m_fanControlTab->fanProfileCombo()->currentText();

  if ( currentName.endsWith(" [Built-in]") )
    currentName = currentName.left( currentName.size() - 11 ); // 11 is length of " [Built-in]"

  // Get the current profile data
  QString json = m_profileManager->getFanProfile( currentProfileId );
  if ( json.isEmpty() ) {
    QMessageBox::warning(this, "Error", "Failed to get fan profile data.");
    return;
  }

  // Generate new name: "New {name}" with optional incrementing number
  QString baseName = QString("New %1").arg(currentName);
  QString profileName = baseName;
  int counter = 1;
  while ( m_fanControlTab->fanProfileCombo()->findText( profileName ) != -1 ) {
    profileName = QString("%1 %2").arg(baseName).arg(counter);
    counter++;
  }

  // Save it under the new name with a new ID
  QString newId = QUuid::createUuid().toString( QUuid::WithoutBraces );
  if ( m_profileManager->setFanProfile( newId, profileName, json ) ) {
    m_fanControlTab->fanProfileCombo()->addItem( profileName, newId );
    if ( m_profileFanProfileCombo && m_profileFanProfileCombo->findText( profileName ) == -1 )
      m_profileFanProfileCombo->addItem( profileName, newId );
    // Select by index so currentIndexChanged fires and editability is updated
    int newIdx = m_fanControlTab->fanProfileCombo()->findData( newId );
    if ( newIdx >= 0 )
      m_fanControlTab->fanProfileCombo()->setCurrentIndex( newIdx );
    statusBar()->showMessage( QString("Copied fan profile to '%1'").arg(profileName) );
  }
  else {
    QMessageBox::warning(this, "Error", "Failed to copy profile to new custom profile.");
  }
}

void MainWindow::onApplyFanProfilesClicked()
{
  if ( not m_fanControlTab->cpuEditor() and not m_fanControlTab->gpuEditor() )
  {
    QMessageBox::warning( this, "No Editors", "No fan curve editors available to apply fan profiles." );
    return;
  }

  if ( !m_UccdClient || !m_UccdClient->isConnected() )
  {
    QMessageBox::warning( this, "Not connected", "Not connected to system service; cannot apply fan profiles." );
    return;
  }

  const auto &cpuPoints = m_fanControlTab->cpuEditor()->points();
  const auto &gpuPoints = m_fanControlTab->gpuEditor()->points();
  QJsonObject root;
  QJsonArray cpuArr;
  QJsonArray gpuArr;

  for ( const auto &p : cpuPoints )
  {
    QJsonObject o;
    o["temp"] = qRound(p.temp);
    o["speed"] = qRound(p.duty);
    cpuArr.append( o );
  }

  for ( const auto &p : gpuPoints )
  {
    QJsonObject o;
    o["temp"] = qRound(p.temp);
    o["speed"] = qRound(p.duty);
    gpuArr.append( o );
  }

  // Water cooler fan points
  QJsonArray wcFanArr;
  if ( m_fanControlTab->wcFanEditor() )
  {
    const auto &wcFanPoints = m_fanControlTab->wcFanEditor()->points();
    for ( const auto &p : wcFanPoints )
    {
      QJsonObject o;
      o["temp"] = qRound(p.temp);
      o["speed"] = qRound(p.duty);
      wcFanArr.append( o );
    }
  }

  // Pump threshold points
  QJsonArray pumpArr;
  if ( m_fanControlTab->pumpEditor() )
  {
    const auto &pumpPoints = m_fanControlTab->pumpEditor()->points();
    for ( const auto &p : pumpPoints )
    {
      QJsonObject o;
      o["temp"] = qRound(p.temp);
      o["speed"] = p.level;
      pumpArr.append( o );
    }
  }

  root[ "cpu" ] = cpuArr;
  root[ "gpu" ] = gpuArr;
  root[ "waterCoolerFan" ] = wcFanArr;
  root[ "pump" ] = pumpArr;
  if ( !m_currentFanProfile.isEmpty() )
    root[ "fanProfileId" ] = m_currentFanProfile;

  QJsonDocument doc( root );
  QString json = QString::fromUtf8( doc.toJson( QJsonDocument::Compact ) );

  if ( m_UccdClient->applyFanProfiles( json.toStdString() ) )
  {
    statusBar()->showMessage( "Temporary fan profiles applied" );

    // Keep an internal copy so UI actions like revert have the current values
    m_cpuFanPoints.clear();
    for ( const auto &p : m_fanControlTab->cpuEditor()->points() )
      m_cpuFanPoints.append( { static_cast< int >( p.temp ), static_cast< int >( p.duty ) } );

    m_gpuFanPoints.clear();
    for ( const auto &p : m_fanControlTab->gpuEditor()->points() )
      m_gpuFanPoints.append( { static_cast< int >( p.temp ), static_cast< int >( p.duty ) } );

    m_waterCoolerFanPoints.clear();
    if ( m_fanControlTab->wcFanEditor() )
      for ( const auto &p : m_fanControlTab->wcFanEditor()->points() )
        m_waterCoolerFanPoints.append( { static_cast< int >( p.temp ), static_cast< int >( p.duty ) } );

    // Re-send the current water cooler enable state to the daemon so that
    // the profile apply (which may set enableWaterCooler from profile data)
    // doesn't override the user's current checkbox state.
    if ( m_fanControlTab )
      m_fanControlTab->sendWaterCoolerEnable( m_fanControlTab->isWaterCoolerEnabled() );
  }
  else
  {
    QMessageBox::warning( this, "Apply Failed", "Failed to apply fan profiles. Check service logs or connection." );
  }
}

void MainWindow::onRevertFanProfilesClicked()
{
  loadFanPoints();
}

void MainWindow::loadFanPoints()
{
  // Load fan profile JSON for the currently selected fan profile (if custom)
  if ( m_currentFanProfile.isEmpty() ) return;

  QString json = m_profileManager->getFanProfile( m_currentFanProfile );
  QJsonDocument doc = QJsonDocument::fromJson(json.toUtf8());
  if (doc.isObject()) {
    QJsonObject obj = doc.object();
    if (obj.contains("tableCPU")) {
      QJsonArray arr = obj["tableCPU"].toArray();
      m_cpuFanPoints.clear();
      QVector<FanCurveEditorWidget::Point> cpuPoints;
      for (const QJsonValue &v : arr) {
        QJsonObject o = v.toObject();
        int temp = o["temp"].toInt();
        int speed = o["speed"].toInt();
        m_cpuFanPoints.append({temp, speed});
        cpuPoints.append({static_cast<double>(temp), static_cast<double>(speed)});
      }
      if (m_fanControlTab->cpuEditor()) {
        m_fanControlTab->cpuEditor()->setPoints(cpuPoints);
      }
    }
    if (obj.contains("tableGPU")) {
      QJsonArray arr = obj["tableGPU"].toArray();
      m_gpuFanPoints.clear();
      QVector<FanCurveEditorWidget::Point> gpuPoints;
      for (const QJsonValue &v : arr) {
        QJsonObject o = v.toObject();
        int temp = o["temp"].toInt();
        int speed = o["speed"].toInt();
        m_gpuFanPoints.append({temp, speed});
        gpuPoints.append({static_cast<double>(temp), static_cast<double>(speed)});
      }
      if (m_fanControlTab->gpuEditor()) {
        m_fanControlTab->gpuEditor()->setPoints(gpuPoints);
      }
    }
    if (obj.contains("tableWaterCoolerFan")) {
      QJsonArray arr = obj["tableWaterCoolerFan"].toArray();
      m_waterCoolerFanPoints.clear();
      QVector<FanCurveEditorWidget::Point> wcFanPoints;
      for (const QJsonValue &v : arr) {
        QJsonObject o = v.toObject();
        int temp = o["temp"].toInt();
        int speed = o["speed"].toInt();
        m_waterCoolerFanPoints.append({temp, speed});
        wcFanPoints.append({static_cast<double>(temp), static_cast<double>(speed)});
      }
      if (m_fanControlTab->wcFanEditor()) {
        m_fanControlTab->wcFanEditor()->setPoints(wcFanPoints);
      }
    }
    if (obj.contains("tablePump")) {
      QJsonArray arr = obj["tablePump"].toArray();
      QVector<PumpCurveEditorWidget::Point> pumpPoints;
      for (const QJsonValue &v : arr) {
        QJsonObject o = v.toObject();
        double temp = o["temp"].toDouble();
        int speed = o["speed"].toInt();
        pumpPoints.append({temp, speed});
      }
      if (m_fanControlTab->pumpEditor() && !pumpPoints.isEmpty()) {
        m_fanControlTab->pumpEditor()->setPoints(pumpPoints);
      }
    }
  }
}


void MainWindow::saveFanPoints()
{
  QJsonObject obj;

  // Get CPU points from the editor
  QJsonArray cpuArr;
  if ( m_fanControlTab->cpuEditor() )
  {
    const auto& cpuPoints = m_fanControlTab->cpuEditor()->points();
    for (const auto &p : cpuPoints) {
      QJsonObject o;
      o["temp"] = p.temp;
      o["speed"] = p.duty;
      cpuArr.append(o);
    }
  }
  obj["tableCPU"] = cpuArr;

  // Get GPU points from the editor
  QJsonArray gpuArr;
  if (m_fanControlTab->gpuEditor()) {
    const auto& gpuPoints = m_fanControlTab->gpuEditor()->points();
    for (const auto &p : gpuPoints) {
      QJsonObject o;
      o["temp"] = p.temp;
      o["speed"] = p.duty;
      gpuArr.append(o);
    }
  }
  obj["tableGPU"] = gpuArr;

  // Get water cooler fan points from the editor
  QJsonArray wcFanArr;
  if (m_fanControlTab->wcFanEditor()) {
    const auto& wcFanPoints = m_fanControlTab->wcFanEditor()->points();
    for (const auto &p : wcFanPoints) {
      QJsonObject o;
      o["temp"] = p.temp;
      o["speed"] = p.duty;
      wcFanArr.append(o);
    }
  }

  obj["tableWaterCoolerFan"] = wcFanArr;

  // Get pump points from the editor
  QJsonArray pumpArr;
  if (m_fanControlTab->pumpEditor()) {
    const auto& pumpPoints = m_fanControlTab->pumpEditor()->points();
    for (const auto &p : pumpPoints) {
      QJsonObject o;
      o["temp"] = p.temp;
      o["speed"] = p.level;
      pumpArr.append(o);
    }
  }
  obj["tablePump"] = pumpArr;

  QJsonDocument doc(obj);
  QString json = doc.toJson(QJsonDocument::Compact);

  const QString currentId = m_fanControlTab->fanProfileCombo() ? m_fanControlTab->fanProfileCombo()->currentData().toString() : QString();
  const QString currentName = m_fanControlTab->fanProfileCombo() ? m_fanControlTab->fanProfileCombo()->currentText() : QString();
  if ( currentId.isEmpty() ) {
    QMessageBox::warning(this, "Save Failed", "No fan profile selected to save to.");
    return;
  }

  if ( m_fanControlTab->builtinFanProfiles().contains( currentId ) ) {
    QMessageBox::warning(this, "Save Failed", "Cannot overwrite built-in fan profile. Copy it to a custom profile first.");
    return;
  }

  // Save into selected custom profile (by ID)
  m_profileManager->setFanProfile( currentId, currentName, json );
}

} // namespace ucc
