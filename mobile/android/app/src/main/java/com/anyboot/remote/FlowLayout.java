package com.anyboot.remote;

import android.content.Context;
import android.view.View;
import android.view.ViewGroup;

/**
 * 自动换行的横向流式布局 —— 原生没有 flex，这就是 flex-wrap 的等价物。
 *
 * <p>按钮行在窄屏 / 不同屏幕宽度上一行放不下时自动折到下一行，
 * 不再溢出屏幕导致后面的按钮点不到（程序化 UI 的屏幕适配兜底）。
 */
public class FlowLayout extends ViewGroup {

    private final int hGap;
    private final int vGap;

    public FlowLayout(Context context, int hGapDp, int vGapDp) {
        super(context);
        float density = context.getResources().getDisplayMetrics().density;
        hGap = Math.round(hGapDp * density);
        vGap = Math.round(vGapDp * density);
    }

    private static int marginHorizontal(LayoutParams lp) {
        return lp instanceof MarginLayoutParams
                ? ((MarginLayoutParams) lp).leftMargin + ((MarginLayoutParams) lp).rightMargin : 0;
    }

    private static int marginVertical(LayoutParams lp) {
        return lp instanceof MarginLayoutParams
                ? ((MarginLayoutParams) lp).topMargin + ((MarginLayoutParams) lp).bottomMargin : 0;
    }

    @Override
    protected void onMeasure(int widthSpec, int heightSpec) {
        int available = MeasureSpec.getSize(widthSpec)
                - getPaddingLeft() - getPaddingRight();
        int widthMode = MeasureSpec.getMode(widthSpec);
        int x = 0;
        int y = 0;
        int rowHeight = 0;
        int maxRowWidth = 0;
        for (int i = 0; i < getChildCount(); i++) {
            View child = getChildAt(i);
            measureChild(child, widthSpec, heightSpec);
            LayoutParams lp = child.getLayoutParams();
            int childWidth = child.getMeasuredWidth() + marginHorizontal(lp);
            int childHeight = child.getMeasuredHeight() + marginVertical(lp);
            if (x > 0 && x + childWidth > available) {
                maxRowWidth = Math.max(maxRowWidth, x - hGap);
                x = 0;
                y += rowHeight + vGap;
                rowHeight = 0;
            }
            x += childWidth + hGap;
            rowHeight = Math.max(rowHeight, childHeight);
        }
        maxRowWidth = Math.max(maxRowWidth, Math.min(x - hGap, available));
        int totalWidth = widthMode == MeasureSpec.EXACTLY
                ? MeasureSpec.getSize(widthSpec)
                : resolveSize(maxRowWidth + getPaddingLeft() + getPaddingRight(), widthSpec);
        int totalHeight = y + rowHeight + getPaddingTop() + getPaddingBottom();
        setMeasuredDimension(totalWidth, resolveSize(totalHeight, heightSpec));
    }

    @Override
    protected void onLayout(boolean changed, int l, int t, int r, int b) {
        int rightEdge = r - l - getPaddingRight();
        int x = getPaddingLeft();
        int y = getPaddingTop();
        int rowHeight = 0;
        for (int i = 0; i < getChildCount(); i++) {
            View child = getChildAt(i);
            LayoutParams lp = child.getLayoutParams();
            int marginLeft = lp instanceof MarginLayoutParams ? ((MarginLayoutParams) lp).leftMargin : 0;
            int marginTop = lp instanceof MarginLayoutParams ? ((MarginLayoutParams) lp).topMargin : 0;
            int childWidth = child.getMeasuredWidth() + marginHorizontal(lp);
            int childHeight = child.getMeasuredHeight() + marginVertical(lp);
            if (x - getPaddingLeft() > 0 && x + childWidth > rightEdge) {
                x = getPaddingLeft();
                y += rowHeight + vGap;
                rowHeight = 0;
            }
            child.layout(x + marginLeft, y + marginTop,
                    x + marginLeft + child.getMeasuredWidth(),
                    y + marginTop + child.getMeasuredHeight());
            x += childWidth + hGap;
            rowHeight = Math.max(rowHeight, childHeight);
        }
    }

    @Override
    protected LayoutParams generateDefaultLayoutParams() {
        return new MarginLayoutParams(
                LayoutParams.WRAP_CONTENT, LayoutParams.WRAP_CONTENT);
    }
}
