// SPDX-License-Identifier: GPL-3.0-or-later
#include "FanCurveEditorWidget.hpp"
#include <QtTest>

class TestFanCurveEditor:public QObject {
  Q_OBJECT
  static void move(QWidget& w,QPoint p) {
    QMouseEvent e(QEvent::MouseMove,p,w.mapToGlobal(p),Qt::NoButton,Qt::LeftButton,Qt::NoModifier);
    QApplication::sendEvent(&w,&e);
  }
private slots:
  void draggingBackRestoresNeighbours() {
    FanCurveEditorWidget w;w.resize(900,396);w.show();
    w.setPoints({{20,20},{40,40},{60,60},{80,80},{100,100}});
    const QPoint start(280,208); // 40 C / 40% in an 800 x 300 plot
    QTest::mousePress(&w,Qt::LeftButton,Qt::NoModifier,start);
    move(w,start-QPoint(0,150));
    QVERIFY(w.points()[2].duty>80);
    move(w,start);
    QTest::mouseRelease(&w,Qt::LeftButton,Qt::NoModifier,start);
    for(int i=0;i<5;++i) QVERIFY(std::abs(w.points()[i].duty-(20+i*20))<0.01);
  }
  void densePointsPickNearestAndReleaseCommits() {
    FanCurveEditorWidget w;w.resize(900,396);w.show();
    QVector<FanCurveEditorWidget::Point> p;
    for(int t=20;t<=100;++t) p.append({double(t),20});
    w.setPoints(p);
    // Adjacent points are 10 pixels apart; the closest must win.
    QTest::mousePress(&w,Qt::LeftButton,Qt::NoModifier,QPoint(288,268));
    QTest::mouseRelease(&w,Qt::LeftButton,Qt::NoModifier,QPoint(288,238));
    QCOMPARE(w.points()[20].duty,20.0);
    QCOMPARE(w.points()[21].duty,30.0);
  }
  void rightClickAndProfileReplacementDoNotDrag() {
    FanCurveEditorWidget w;w.resize(900,396);w.show();
    const QVector<FanCurveEditorWidget::Point> p{{20,20},{40,40},{60,60}};
    w.setPoints(p);
    QSignalSpy changed(&w,&FanCurveEditorWidget::pointsChanged);
    QTest::mousePress(&w,Qt::RightButton,Qt::NoModifier,QPoint(280,208));
    move(w,QPoint(280,100));
    QTest::mouseRelease(&w,Qt::RightButton,Qt::NoModifier,QPoint(280,100));
    QCOMPARE(changed.size(),0);
    QTest::mousePress(&w,Qt::LeftButton,Qt::NoModifier,QPoint(280,208));
    w.setPoints({{20,10}});changed.clear();
    move(w,QPoint(280,100));
    QCOMPARE(changed.size(),0);
    QCOMPARE(w.points()[0].duty,10.0);
  }
};
QTEST_MAIN(TestFanCurveEditor)
#include "test_fan_curve_editor.moc"
