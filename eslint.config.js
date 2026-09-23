import eslint from "@eslint/js";
import globals from "globals";
import tseslint from "typescript-eslint";
import vue from "eslint-plugin-vue";

export default tseslint.config(
  {
    // Tauri/Rust 可能在仓库根目录生成 target；构建产物不应进入前端类型化 lint。
    ignores: [
      "dist/**",
      "target/**",
      "node_modules/**",
      "src-tauri/target/**",
      // 业务归档仅用于恢复，不参与当前框架的类型化 lint。
      ".backup/**",
      // 本地采集与分析草稿；与 .backup 同类，不是构建输入。
      ".scratch/**",
      "tools/**",
      "eslint.config.js",
    ],
  },
  eslint.configs.recommended,
  ...tseslint.configs.recommendedTypeChecked,
  ...vue.configs["flat/recommended"],
  {
    files: ["**/*.{ts,vue}"],
    languageOptions: {
      globals: globals.browser,
      parserOptions: {
        parser: tseslint.parser,
        projectService: true,
        tsconfigRootDir: import.meta.dirname,
        extraFileExtensions: [".vue"],
      },
    },
    rules: {
      "vue/multi-word-component-names": "off",
      // 布局换行交给 Prettier 统一处理，避免两套格式规则相互冲突。
      "vue/max-attributes-per-line": "off",
      "vue/singleline-html-element-content-newline": "off",
      "vue/html-self-closing": "off",
      // 模板闭合括号与缩进同样由 Prettier 负责，避免长属性被两套规则反复改写。
      "vue/html-closing-bracket-newline": "off",
      "vue/html-indent": "off",
      "@typescript-eslint/consistent-type-imports": "error",
      "@typescript-eslint/no-floating-promises": "error",
    },
  },
);
