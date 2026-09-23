<template>
  <!-- 更新前固定展示备份提醒；它不阻断更新，只负责把数据保护事项明确告知用户。 -->
  <aside class="update-backup-notice" role="note" aria-label="更新前备份建议">
    <strong class="update-backup-notice__title">更新前建议先备份</strong>
    <p>为避免更新过程中断或电脑异常导致数据风险，建议先完成以下备份：</p>
    <ul>
      <li>应用数据和 SQLite 数据库。</li>
      <li>御魂评分标准、分析规则和自定义方案。</li>
      <li>重要的原始导入文件、历史记录和导出文件。</li>
    </ul>
    <p class="update-backup-notice__footnote">
      正常更新不会主动删除用户数据；备份完成后再点击更新更安全。
    </p>
  </aside>
</template>
