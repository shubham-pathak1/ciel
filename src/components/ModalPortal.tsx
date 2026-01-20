/**
 * @file ModalPortal.tsx
 * @description Utility component for rendering overlays outside the standard React DOM hierarchy.
 */

import React from 'react';
import { createPortal } from 'react-dom';

interface ModalPortalProps {
    children: React.ReactNode;
}

/**
 * ModalPortal Component.
 * 
 * Responsibilities:
 * - Teleports children into `document.body`.
 * - Escapes parent CSS stacking contexts (z-index, overflow, transform) to 
 *   ensure modals always appear on top.
 */
export const ModalPortal: React.FC<ModalPortalProps> = ({ children }) => {
    return createPortal(
        children,
        document.body
    );
};
