#!/bin/bash
# 全功能抽查（4 轮共 44 项）
export NVM_DIR="$HOME/.nvm"; . "$NVM_DIR/nvm.sh"
cd /tmp/cmp
echo "########## 第 1 轮：hub 主交互 ##########"; node func1.js 2>&1 | tail -10
echo "########## 第 2 轮：服务器/传输/密钥/备份 ##########"; node func2.js 2>&1 | tail -10
echo "########## 第 3 轮：4 个独立窗 ##########"; node func3.js 2>&1 | tail -14
echo "########## 第 4 轮：弹层内部交互 ##########"; node func4.js 2>&1 | tail -8
echo "########## 第 5 轮：主窗 4 页签 + 设置项 ##########"; node func5.js 2>&1 | tail -13
echo "########## 主题渲染 ##########"; node theme-render.js 2>&1 | tail -9
echo "########## 主题功能 ##########"; node theme-func.js 2>&1 | tail -16
